use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use isola::{
    host::{BoxError, LogContext, LogLevel, NoopOutputSink, OutputSink},
    sandbox::{
        Arg, CallOutput, DirPerms, Error as IsolaError, FilePerms, Sandbox, SandboxOptions, args,
    },
};
use parking_lot::Mutex;
use tempfile::tempdir;

use super::common::{TestHost, build_module, build_module_with_max_memory};

const CAP_NEIGHBORHOOD_BYTES: usize = 1024 * 1024;
const MEMORY_CAP_BYTES: usize = 64 * 1024 * 1024;
const LARGE_STDOUT_BYTES: usize = 256 * 1024;

struct CollectLogsSink {
    logs: Arc<Mutex<Vec<(String, String)>>>,
}

impl CollectLogsSink {
    const fn new(logs: Arc<Mutex<Vec<(String, String)>>>) -> Self {
        Self { logs }
    }
}

#[async_trait::async_trait]
impl OutputSink for CollectLogsSink {
    async fn on_item(&self, _value: isola::value::Value) -> std::result::Result<(), BoxError> {
        Ok(())
    }

    async fn on_complete(
        &self,
        _value: Option<isola::value::Value>,
    ) -> std::result::Result<(), BoxError> {
        Ok(())
    }

    async fn on_log(
        &self,
        level: LogLevel,
        _log_context: LogContext<'_>,
        message: &str,
    ) -> std::result::Result<(), BoxError> {
        self.logs
            .lock()
            .push((level.as_str().to_string(), message.to_string()));
        Ok(())
    }
}

async fn call_with_timeout<I>(
    sandbox: &mut Sandbox<TestHost>,
    function: &str,
    args: I,
    timeout: Duration,
) -> std::result::Result<CallOutput, IsolaError>
where
    I: IntoIterator<Item = Arg>,
{
    tokio::time::timeout(timeout, sandbox.call(function, args))
        .await
        .unwrap_or_else(|_| {
            Err(IsolaError::Runtime(anyhow::anyhow!(
                "sandbox call timed out after {}ms",
                timeout.as_millis()
            )))
        })
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_eval_and_call_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main():\n\tprint('trace-print')\n\treturn 42",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .context("failed to call function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: i64 = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode end output")?;
    assert_eq!(value, 42);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_call_with_sink_does_not_retain_refs() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\treturn 42", NoopOutputSink::shared())
        .await
        .context("failed to evaluate script")?;

    let sink: Arc<dyn OutputSink> = Arc::new(NoopOutputSink);
    let initial = Arc::strong_count(&sink);
    assert_eq!(initial, 1, "unexpected initial sink refcount");

    sandbox
        .call_with_sink("main", [], Arc::clone(&sink))
        .await
        .context("failed to call function with sink")?;
    assert_eq!(
        Arc::strong_count(&sink),
        initial,
        "sink refcount changed after call_with_sink",
    );

    sandbox
        .call_with_sink("main", [], Arc::clone(&sink))
        .await
        .context("failed to call function with sink on second call")?;
    assert_eq!(
        Arc::strong_count(&sink),
        initial,
        "sink refcount changed after repeated call_with_sink",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_streaming_output() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main():\n\tfor i in range(3):\n\t\tyield i",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate streaming script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .context("failed to call streaming function")?;

    assert_eq!(output.items.len(), 3, "expected three partial outputs");
    let mut values = Vec::with_capacity(output.items.len());
    for item in &output.items {
        values.push(
            item.to_serde::<i64>()
                .context("failed to decode partial output")?,
        );
    }
    assert_eq!(values, vec![0, 1, 2]);

    assert!(output.result.is_none(), "expected null end output");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_eval_script_logs_to_sink() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = CollectLogsSink::new(logs.clone());
    match tokio::time::timeout(
        Duration::from_secs(2),
        sandbox.eval_script(
            "print('eval-stdout')\nimport sandbox.logging\nsandbox.logging.info('eval-log')",
            Arc::new(sink),
        ),
    )
    .await
    {
        Ok(result) => result.context("failed to evaluate script")?,
        Err(_) => {
            return Err(anyhow::anyhow!("sandbox eval timed out after {}ms", 2_000));
        }
    }
    {
        let logs = logs.lock();

        assert!(
            logs.iter()
                .any(|(context, message)| context == "stdout" && message.contains("eval-stdout")),
            "expected eval stdout log in sink, logs: {:?}",
            *logs
        );
        assert!(
            logs.iter()
                .any(|(context, message)| context == "info" && message.contains("eval-log")),
            "expected eval logging event in sink, logs: {:?}",
            *logs
        );
        drop(logs);
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_large_stdout_output_is_not_truncated() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            format!(
                "def main():\n\
                 \tpayload = 'x' * {LARGE_STDOUT_BYTES}\n\
                 \tprint(payload, end='')\n\
                 \treturn len(payload)"
            ),
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate large stdout script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(10))
        .await
        .context("failed to call large stdout function")?;

    let emitted_len: usize = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode end output")?;
    assert_eq!(emitted_len, LARGE_STDOUT_BYTES);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_argument_cbor_path() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main(i, s):\n\treturn (i + 1, s.upper())",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate argument script")?;

    let args = args![41_i64, s = "hello"]?;
    let output = call_with_timeout(&mut sandbox, "main", args, Duration::from_secs(2))
        .await
        .context("failed to call argument function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: (i64, String) = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode argument result")?;
    assert_eq!(value, (42, "HELLO".to_string()));
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_reinstantiate_smoke() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    for expected in [7_i64, 11_i64] {
        let mut sandbox = module
            .instantiate(TestHost::default(), SandboxOptions::default())
            .await
            .context("failed to instantiate sandbox")?;

        sandbox
            .eval_script(
                format!("def main():\n\treturn {expected}"),
                NoopOutputSink::shared(),
            )
            .await
            .context("failed to evaluate script")?;

        let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
            .await
            .context("failed to call function")?;
        let value: i64 = output
            .result
            .as_ref()
            .context("expected exactly one end output")?
            .to_serde()
            .context("failed to decode roundtrip output")?;
        assert_eq!(value, expected);
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_guest_exception_surface() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main():\n\traise RuntimeError(\"boom\")",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate exception script")?;

    let err = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .expect_err("expected exception from guest function");
    let IsolaError::Guest { message } = err else {
        panic!("expected guest error, got {err:?}");
    };
    assert!(
        message.contains("boom"),
        "unexpected error message: {message}",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_state_persists_within_sandbox() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "counter = 0\n\
             def main():\n\
             \tglobal counter\n\
             \tcounter += 1\n\
             \treturn counter",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate stateful script")?;

    let first = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .context("failed first stateful call")?;
    let second = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .context("failed second stateful call")?;

    let first_v: i64 = first
        .result
        .as_ref()
        .context("expected exactly one first end output")?
        .to_serde()
        .context("failed to decode first value")?;
    let second_v: i64 = second
        .result
        .as_ref()
        .context("expected exactly one second end output")?
        .to_serde()
        .context("failed to decode second value")?;
    assert_eq!(first_v, 1);
    assert_eq!(second_v, 2);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_call_timeout() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main():\n\twhile True:\n\t\tpass",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate timeout script")?;

    let err = call_with_timeout(&mut sandbox, "main", [], Duration::from_millis(1))
        .await
        .expect_err("expected timeout while executing guest function");
    let IsolaError::Runtime(cause) = err else {
        panic!("expected runtime timeout error, got {err:?}");
    };
    let message = cause.to_string().to_ascii_lowercase();
    assert!(
        message.contains("timeout") || message.contains("timed out"),
        "unexpected timeout error message: {cause}"
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_memory_limiter_is_enforced() -> Result<()> {
    let Some(module) = build_module_with_max_memory(MEMORY_CAP_BYTES).await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main():\n\
             \tchunks = []\n\
             \tfor _ in range(1024):\n\
             \t\tchunks.append(bytes(1024 * 1024))\n\
             \treturn len(chunks)",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate memory pressure script")?;

    let memory_before = sandbox.memory_usage();
    let err = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(10))
        .await
        .expect_err("expected memory limit error while allocating guest memory");
    let memory_after = sandbox.memory_usage();

    let message = match err {
        IsolaError::Guest { message } => message.to_ascii_lowercase(),
        IsolaError::Runtime(cause) => cause.to_string().to_ascii_lowercase(),
    };
    assert!(
        message.contains("memory")
            || message.contains("grow")
            || message.contains("alloc")
            || message.contains("oom"),
        "unexpected memory limit error message: {message}",
    );

    assert!(
        memory_after >= memory_before,
        "expected memory usage to grow during allocation, before={memory_before}, after={memory_after}",
    );
    assert!(
        memory_after <= MEMORY_CAP_BYTES,
        "memory usage exceeded configured cap: used={memory_after}, cap={MEMORY_CAP_BYTES}",
    );
    assert!(
        memory_after >= MEMORY_CAP_BYTES.saturating_sub(CAP_NEIGHBORHOOD_BYTES),
        "expected usage to reach memory cap neighborhood, used={memory_after}, cap={MEMORY_CAP_BYTES}",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_writable_directory_mapping_filesystem_roundtrip() -> Result<()> {
    let temp = tempdir().context("failed to create temp directory")?;
    let mapped_dir = temp.path().to_path_buf();

    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut options = SandboxOptions::default();
    options.mount(
        &mapped_dir,
        "/fs",
        DirPerms::READ | DirPerms::MUTATE,
        FilePerms::READ | FilePerms::WRITE,
    );
    let mut sandbox = module
        .instantiate(TestHost::default(), options)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "def main(text):\n\
             \tpath = '/fs/output.txt'\n\
             \twith open(path, 'w', encoding='utf-8') as fh:\n\
             \t\tfh.write(text)\n\
             \twith open(path, 'r', encoding='utf-8') as fh:\n\
             \t\treturn fh.read()",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate filesystem script")?;

    let args = args!["hello-fs"]?;
    let output = call_with_timeout(&mut sandbox, "main", args, Duration::from_secs(2))
        .await
        .context("failed to call filesystem function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let result: String = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode filesystem result")?;
    assert_eq!(result, "hello-fs");

    let host_file = mapped_dir.join("output.txt");
    let host_contents = std::fs::read_to_string(&host_file).with_context(|| {
        format!(
            "failed to read mapped host file after guest write: {}",
            host_file.display()
        )
    })?;
    assert_eq!(host_contents, "hello-fs");

    Ok(())
}
