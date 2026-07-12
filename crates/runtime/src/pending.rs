use std::{
    cell::RefCell,
    fmt,
    time::{Duration, Instant},
};

use futures::future::join_all;

use crate::{
    Deadline, block_on,
    isola::script::host,
    wasi_http::{self, HttpRequest, HttpResponse},
};

/// The completed value of a deferred runtime operation.
pub enum Output {
    Host(Result<Vec<u8>, String>),
    Http {
        request_url: String,
        response: Result<HttpResponse, String>,
    },
    Sleep,
}

/// The state of an operation removed from the registry.
pub enum Take {
    Ready(Output),
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidHandle(u32);

impl InvalidHandle {
    #[must_use]
    pub const fn handle(self) -> u32 {
        self.0
    }
}

impl fmt::Display for InvalidHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid or already-consumed handle: {}", self.0)
    }
}

impl std::error::Error for InvalidHandle {}

enum Operation {
    Host {
        call_type: String,
        payload: Vec<u8>,
        result: Option<Result<Vec<u8>, String>>,
    },
    Http {
        request: HttpRequest,
        result: Option<Result<HttpResponse, String>>,
    },
    Sleep(Deadline),
}

impl Operation {
    fn is_ready(&self, now: Instant) -> bool {
        match self {
            Self::Host { result, .. } => result.is_some(),
            Self::Http { result, .. } => result.is_some(),
            Self::Sleep(deadline) => deadline.is_ready_at(now),
        }
    }
}

thread_local! {
    static OPERATIONS: RefCell<Vec<Option<Operation>>> = const { RefCell::new(Vec::new()) };
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a WebAssembly guest cannot allocate enough slots to overflow u32"
)]
const fn handle(index: usize) -> u32 {
    index as u32
}

fn register(operation: Operation) -> u32 {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        if let Some((index, slot)) = operations
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        {
            *slot = Some(operation);
            handle(index)
        } else {
            let index = operations.len();
            operations.push(Some(operation));
            handle(index)
        }
    })
}

/// Register a deferred hostcall.
#[must_use]
pub fn register_hostcall(call_type: String, payload: Vec<u8>) -> u32 {
    register(Operation::Host {
        call_type,
        payload,
        result: None,
    })
}

/// Register a deferred request sent through `wasi:http/client`.
#[must_use]
pub fn register_http(request: HttpRequest) -> u32 {
    register(Operation::Http {
        request,
        result: None,
    })
}

/// Register a sleep. `None` represents an operation that is ready immediately.
#[must_use]
pub fn register_sleep(duration: Option<Duration>) -> u32 {
    register(Operation::Sleep(
        duration.map_or_else(Deadline::default, Deadline::after),
    ))
}

/// Return all handles whose operation can be consumed without blocking.
#[must_use]
pub fn ready_handles() -> Vec<u32> {
    let now = Instant::now();
    OPERATIONS.with(|operations| {
        operations
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .filter(|operation| operation.is_ready(now))
                    .map(|_| handle(index))
            })
            .collect()
    })
}

/// Return the earliest deadline among registered sleeps.
#[must_use]
pub fn next_deadline() -> Option<Instant> {
    OPERATIONS.with(|operations| {
        operations
            .borrow()
            .iter()
            .filter_map(|slot| match slot {
                Some(Operation::Sleep(deadline)) => deadline.ready_at(),
                _ => None,
            })
            .min()
    })
}

/// Return whether the registry contains any operation.
#[must_use]
pub fn has_pending() -> bool {
    OPERATIONS.with(|operations| operations.borrow().iter().any(Option::is_some))
}

/// Execute all registered hostcalls and HTTP requests that have not been
/// driven.
///
/// Calls are issued concurrently in one executor turn. Returns `true` when at
/// least one operation was driven.
#[must_use]
pub fn drive_pending() -> bool {
    enum Request {
        Host(String, Vec<u8>),
        Http(HttpRequest),
    }

    enum Response {
        Host(Result<Vec<u8>, String>),
        Http(Result<HttpResponse, String>),
    }

    let requests: Vec<(usize, Request)> = OPERATIONS.with(|operations| {
        operations
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Some(Operation::Host {
                    call_type,
                    payload,
                    result: None,
                }) => Some((index, Request::Host(call_type.clone(), payload.clone()))),
                Some(Operation::Http {
                    request,
                    result: None,
                }) => Some((index, Request::Http(request.clone()))),
                _ => None,
            })
            .collect()
    });
    if requests.is_empty() {
        return false;
    }

    let responses = block_on(join_all(requests.into_iter().map(
        |(index, request)| async move {
            let response = match request {
                Request::Host(call_type, payload) => {
                    Response::Host(host::hostcall(call_type, payload).await)
                }
                Request::Http(request) => Response::Http(wasi_http::send(request).await),
            };
            (index, response)
        },
    )));

    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        for (index, response) in responses {
            match (operations.get_mut(index), response) {
                (Some(Some(Operation::Host { result, .. })), Response::Host(response)) => {
                    *result = Some(response);
                }
                (Some(Some(Operation::Http { result, .. })), Response::Http(response)) => {
                    *result = Some(response);
                }
                _ => {}
            }
        }
    });
    true
}

fn take_operation(handle: u32) -> Result<Operation, InvalidHandle> {
    OPERATIONS.with(|operations| {
        operations
            .borrow_mut()
            .get_mut(handle as usize)
            .and_then(Option::take)
            .ok_or(InvalidHandle(handle))
    })
}

/// Remove an operation and return its completed output, if ready.
///
/// An undriven hostcall or HTTP request is removed and reported as pending.
///
/// # Errors
///
/// Returns [`InvalidHandle`] if the handle is out of range or was consumed.
pub fn take(handle: u32) -> Result<Take, InvalidHandle> {
    Ok(match take_operation(handle)? {
        Operation::Host {
            result: Some(result),
            ..
        } => Take::Ready(Output::Host(result)),
        Operation::Http {
            request,
            result: Some(response),
        } => Take::Ready(Output::Http {
            request_url: request.url().to_string(),
            response,
        }),
        Operation::Sleep(_) => Take::Ready(Output::Sleep),
        Operation::Host { result: None, .. } | Operation::Http { result: None, .. } => {
            Take::Pending
        }
    })
}

/// Remove one operation, driving it synchronously when necessary.
///
/// # Errors
///
/// Returns [`InvalidHandle`] if the handle is out of range or was consumed.
pub fn drive_one(handle: u32) -> Result<Output, InvalidHandle> {
    Ok(match take_operation(handle)? {
        Operation::Host {
            call_type,
            payload,
            result,
        } => Output::Host(result.unwrap_or_else(|| block_on(host::hostcall(call_type, payload)))),
        Operation::Http { request, result } => {
            let request_url = request.url().to_string();
            let response = result.unwrap_or_else(|| block_on(wasi_http::send(request)));
            Output::Http {
                request_url,
                response,
            }
        }
        Operation::Sleep(_) => Output::Sleep,
    })
}

/// Remove an operation without consuming its output.
pub fn release(handle: u32) {
    OPERATIONS.with(|operations| {
        if let Some(slot) = operations.borrow_mut().get_mut(handle as usize) {
            *slot = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Output, Take};

    #[test]
    fn released_slots_are_reused() {
        let released = super::register_sleep(None);
        let retained = super::register_sleep(None);
        super::release(released);

        let reused = super::register_sleep(None);
        assert_eq!(reused, released);
        assert!(matches!(
            super::take(reused),
            Ok(Take::Ready(Output::Sleep))
        ));
        assert!(matches!(
            super::take(retained),
            Ok(Take::Ready(Output::Sleep))
        ));
    }

    #[test]
    fn consumed_handles_are_invalid() {
        let handle = super::register_sleep(None);
        assert!(matches!(
            super::take(handle),
            Ok(Take::Ready(Output::Sleep))
        ));
        let Err(error) = super::take(handle) else {
            panic!("consumed handle should be rejected");
        };
        assert_eq!(error.handle(), handle);
    }
}
