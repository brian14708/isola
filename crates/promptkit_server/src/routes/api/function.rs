use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{types::Uuid, PgPool};

use crate::{
    model::{FunctionPermission, FunctionVisibility},
    routes::{auth::AuthSession, AppState, Result},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", put(create_function).get(list_functions))
        .route("/:id_or_endpoint", get(get_function))
}

#[derive(Deserialize)]
struct CreateFunction {
    endpoint: Option<String>,
    name: String,
    visibility: FunctionVisibility,
}

async fn create_function(
    auth: AuthSession,
    State(pool): State<PgPool>,
    Json(params): Json<CreateFunction>,
) -> Result {
    let e = sqlx::query!(
        "WITH inserted AS (INSERT INTO promptkit.functions (endpoint, name, visibility) VALUES ($1, $2, $3) RETURNING id) INSERT INTO promptkit.users_functions (function_id, user_id, permission) SELECT id, $4, 'owner' FROM inserted RETURNING function_id",
        params.endpoint,
        params.name,
        params.visibility as FunctionVisibility,
        auth.user_id
    )
    .fetch_one(&pool)
    .await?;
    Ok(Json(json!({
        "id": e.function_id
    }))
    .into_response())
}

#[derive(Deserialize)]
struct ListFunctions {
    offset: Option<i64>,
    count: Option<i64>,
}

async fn list_functions(
    auth: AuthSession,
    Query(params): Query<ListFunctions>,
    State(pool): State<PgPool>,
) -> Result {
    #[derive(Serialize)]
    struct Function {
        #[serde(skip_serializing)]
        total: i64,
        id: Uuid,
        endpoint: Option<String>,
        name: String,
        visibility: FunctionVisibility,
    }

    let e = sqlx::query_as!(
        Function,
        r#"SELECT COUNT(1) OVER() as "total!", id, endpoint, name,
                visibility AS "visibility: FunctionVisibility"
            FROM promptkit.functions
            JOIN promptkit.users_functions
                ON functions.id = users_functions.function_id
            WHERE user_id = $1
            ORDER BY functions.created_at DESC
            OFFSET $2 LIMIT $3"#,
        auth.user_id,
        params.offset.unwrap_or(0),
        params.count.unwrap_or(5)
    )
    .fetch_all(&pool)
    .await?;

    Ok(
        Json(json!({ "functions": e, "total": e.first().map(|f| f.total).unwrap_or(0) }))
            .into_response(),
    )
}

async fn check_access(
    function_id_or_endpoint: &str,
    permission: FunctionPermission,
    user_id: Option<Uuid>,
    pool: &PgPool,
) -> Option<Uuid> {
    struct Function {
        id: Uuid,
        visibility: FunctionVisibility,
    }

    let f: Function = match if let Ok(uuid) = Uuid::parse_str(function_id_or_endpoint) {
        sqlx::query_as!(Function,
            r#"SELECT id, visibility AS "visibility: FunctionVisibility" FROM promptkit.functions WHERE id = $1"#,
            uuid
        )
        .fetch_one(pool)
        .await
    } else {
        sqlx::query_as!(Function,
            r#"SELECT id, visibility AS "visibility: FunctionVisibility" FROM promptkit.functions WHERE endpoint = $1"#,
            function_id_or_endpoint
        )
        .fetch_one(pool)
        .await
    } {
        Ok(f) => f,
        Err(_) => return None,
    };

    match (permission, f.visibility, user_id) {
        (FunctionPermission::Viewer, FunctionVisibility::Public, _) => true,
        (FunctionPermission::Viewer, FunctionVisibility::Internal, Some(_)) => true,
        (_, _, Some(id)) => {
            sqlx::query!(
                r#"SELECT permission AS "permission: FunctionPermission" FROM promptkit.users_functions WHERE function_id = $1 AND user_id = $2"#,
                f.id,
                id
            ).fetch_one(pool).await.map_or(false, |m| m.permission >= permission)
        }
        _ => false,
    }.then_some(f.id)
}

async fn get_function(
    auth: Option<AuthSession>,
    State(pool): State<PgPool>,
    Path(id_or_endpoint): Path<String>,
) -> Result {
    let f = match check_access(
        &id_or_endpoint,
        FunctionPermission::Viewer,
        auth.map(|a| a.user_id),
        &pool,
    )
    .await
    {
        Some(f) => f,
        None => return Ok(StatusCode::FORBIDDEN.into_response()),
    };
    println!("{:?}", f);
    Ok(Json(json!({})).into_response())
}
