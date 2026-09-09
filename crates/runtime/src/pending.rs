use std::{
    cell::RefCell,
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque},
    fmt,
    rc::Rc,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use futures::{
    FutureExt, SinkExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, Abortable, Either, LocalBoxFuture, select},
    pin_mut,
    stream::FuturesUnordered,
};
use parking_lot::Mutex;

use crate::{
    Deadline, block_on,
    isola::script::host,
    wasi::clocks::monotonic_clock,
    wasi_http::{self, HttpBodyStream, HttpRequest, HttpResponse},
};

/// The completed value of a deferred runtime operation.
pub enum Output {
    Host(Result<Vec<u8>, String>),
    Http {
        request_url: String,
        response: Result<HttpResponse, String>,
    },
    Sleep,
    HttpStream(Result<Option<Vec<u8>>, String>),
    HttpUploadWrite(Result<(), String>),
}

struct HttpStream {
    source: HttpBodyStream,
    cancelled: bool,
}

type HttpUploadSender = Rc<Mutex<mpsc::Sender<Result<Vec<u8>, String>>>>;

const UPLOAD_CLOSED: &str = "HTTP request body stream is closed";

struct HttpUpload {
    sender: HttpUploadSender,
    receiver: Option<mpsc::Receiver<Result<Vec<u8>, String>>>,
}

struct HttpUploadWrite {
    sender: HttpUploadSender,
    chunk: Result<Vec<u8>, String>,
}

impl HttpStream {
    fn new(source: HttpBodyStream) -> Self {
        Self {
            source,
            cancelled: false,
        }
    }
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

struct HostRequest {
    call_type: String,
    payload: Vec<u8>,
}

enum State<Request, Response> {
    Deferred(Request),
    Running,
    Ready(Response),
}

impl<Request, Response> State<Request, Response> {
    fn start(&mut self) -> Option<Request> {
        if !matches!(self, Self::Deferred(_)) {
            return None;
        }
        let Self::Deferred(request) = std::mem::replace(self, Self::Running) else {
            unreachable!("deferred state changed while starting operation");
        };
        Some(request)
    }

    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

enum Operation {
    Host(State<HostRequest, Result<Vec<u8>, String>>),
    Http {
        request_url: String,
        state: State<HttpRequest, Result<HttpResponse, String>>,
    },
    Sleep(Deadline),
    HttpStreamRead(State<u32, Result<Option<Vec<u8>>, String>>),
    HttpUploadWrite(State<HttpUploadWrite, Result<(), String>>),
}

impl Operation {
    fn is_ready(&self, now: Instant) -> bool {
        match self {
            Self::Host(state) => state.is_ready(),
            Self::Http { state, .. } => state.is_ready(),
            Self::Sleep(deadline) => deadline.is_ready_at(now),
            Self::HttpStreamRead(state) => state.is_ready(),
            Self::HttpUploadWrite(state) => state.is_ready(),
        }
    }
}

enum Request {
    Host(HostRequest),
    Http(HttpRequest),
    HttpStreamRead(u32),
    HttpUploadWrite(HttpUploadWrite),
}

enum Response {
    Host(Result<Vec<u8>, String>),
    Http(Result<HttpResponse, String>),
    HttpStream(Result<Option<Vec<u8>>, String>),
    HttpUploadWrite(Result<(), String>),
}

enum Completion {
    Operation(u32, Option<Response>),
    Deadline,
}

type InFlight = LocalBoxFuture<'static, Completion>;
type InFlightSet = FuturesUnordered<InFlight>;
// Repeated immediate callbacks must periodically yield the component task so
// imported futures can advance without making ready guest work wait on I/O.
const FAIRNESS_YIELD: Duration = Duration::from_millis(1);

struct Registry {
    operations: HashMap<u32, Operation>,
    in_flight: InFlightSet,
    abort_handles: HashMap<u32, AbortHandle>,
    deferred: VecDeque<u32>,
    ready: Vec<u32>,
    ready_indexes: HashMap<u32, usize>,
    deadlines: BinaryHeap<Reverse<(Instant, u64, u32)>>,
    next_handle: u32,
    next_sequence: u64,
    ready_generation: u64,
    streams: HashMap<u32, HttpStream>,
    uploads: HashMap<u32, HttpUpload>,
}

impl Registry {
    fn new() -> Self {
        Self {
            operations: HashMap::new(),
            in_flight: InFlightSet::new(),
            abort_handles: HashMap::new(),
            deferred: VecDeque::new(),
            ready: Vec::new(),
            ready_indexes: HashMap::new(),
            deadlines: BinaryHeap::new(),
            next_handle: 0,
            next_sequence: 0,
            ready_generation: 0,
            streams: HashMap::new(),
            uploads: HashMap::new(),
        }
    }

    fn allocate_handle(&mut self) -> u32 {
        let start = self.next_handle;
        loop {
            let handle = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if !self.operations.contains_key(&handle)
                && !self.streams.contains_key(&handle)
                && !self.uploads.contains_key(&handle)
            {
                return handle;
            }
            assert_ne!(
                self.next_handle, start,
                "pending operation handle space exhausted"
            );
        }
    }

    fn insert(&mut self, operation: Operation) -> u32 {
        let handle = self.allocate_handle();
        let ready = operation.is_ready(Instant::now());
        let (deferred, deadline) = match &operation {
            Operation::Sleep(deadline) => (false, Some(*deadline)),
            Operation::Host(_)
            | Operation::Http { .. }
            | Operation::HttpStreamRead(_)
            | Operation::HttpUploadWrite(_) => (true, None),
        };
        self.operations.insert(handle, operation);
        if deferred {
            self.deferred.push_back(handle);
        }
        if ready {
            self.mark_ready(handle);
        }
        if let Some(deadline) = deadline {
            if let Some(ready_at) = deadline.ready_at() {
                let sequence = self.next_sequence;
                self.next_sequence = self.next_sequence.wrapping_add(1);
                self.deadlines.push(Reverse((ready_at, sequence, handle)));
                self.refresh_deadlines(Instant::now());
            } else if deadline.is_ready() {
                self.mark_ready(handle);
            }
        }
        handle
    }

    fn start_deferred(&mut self, in_flight: &InFlightSet) {
        while let Some(handle) = self.deferred.pop_front() {
            let request = self
                .operations
                .get_mut(&handle)
                .and_then(|operation| match operation {
                    Operation::Host(state) => state.start().map(Request::Host),
                    Operation::Http { state, .. } => state.start().map(Request::Http),
                    Operation::HttpStreamRead(state) => state.start().map(Request::HttpStreamRead),
                    Operation::HttpUploadWrite(state) => {
                        state.start().map(Request::HttpUploadWrite)
                    }
                    Operation::Sleep(_) => None,
                });
            let Some(request) = request else {
                continue;
            };

            let (abort_handle, abort_registration) = AbortHandle::new_pair();
            self.abort_handles.insert(handle, abort_handle);
            let future = async move {
                match request {
                    Request::Host(HostRequest { call_type, payload }) => {
                        Response::Host(host::hostcall(call_type, payload).await)
                    }
                    Request::Http(request) => Response::Http(wasi_http::send(request).await),
                    Request::HttpStreamRead(handle) => {
                        Response::HttpStream(read_http_stream_async(handle).await)
                    }
                    Request::HttpUploadWrite(HttpUploadWrite { sender, chunk }) => {
                        let mut sender = sender.lock().clone();
                        Response::HttpUploadWrite(
                            sender
                                .send(chunk)
                                .await
                                .map_err(|_| UPLOAD_CLOSED.to_string()),
                        )
                    }
                }
            };
            in_flight.push(
                async move {
                    Completion::Operation(
                        handle,
                        Abortable::new(future, abort_registration).await.ok(),
                    )
                }
                .boxed_local(),
            );
        }
    }

    fn take_in_flight(&mut self) -> InFlightSet {
        std::mem::replace(&mut self.in_flight, InFlightSet::new())
    }

    fn suspend(&mut self, in_flight: InFlightSet) {
        self.in_flight = in_flight;
    }

    fn mark_ready(&mut self, handle: u32) {
        if self.operations.contains_key(&handle) && !self.ready_indexes.contains_key(&handle) {
            self.ready_indexes.insert(handle, self.ready.len());
            self.ready.push(handle);
            self.ready_generation = self.ready_generation.wrapping_add(1);
        }
    }

    fn refresh_deadlines(&mut self, now: Instant) {
        while let Some(Reverse((ready_at, _sequence, handle))) = self.deadlines.peek().copied() {
            let active = self.operations.get(&handle).is_some_and(|operation| {
                matches!(operation, Operation::Sleep(deadline) if deadline.ready_at() == Some(ready_at))
            });
            if !active {
                self.deadlines.pop();
                continue;
            }
            if ready_at > now {
                break;
            }
            self.deadlines.pop();
            self.mark_ready(handle);
        }
    }

    fn refresh_ready(&mut self, now: Instant) {
        self.refresh_deadlines(now);
    }

    fn ready_handles(&mut self, now: Instant) -> Vec<u32> {
        self.refresh_ready(now);
        self.ready.clone()
    }

    fn next_deadline(&mut self, now: Instant) -> Option<Instant> {
        self.refresh_deadlines(now);
        self.deadlines
            .peek()
            .map(|Reverse((ready_at, _, _))| *ready_at)
    }

    fn has_never_deadline(&self) -> bool {
        self.operations
            .values()
            .any(|operation| matches!(operation, Operation::Sleep(deadline) if deadline.is_never()))
    }

    fn complete(&mut self, handle: u32, response: Response) {
        self.abort_handles.remove(&handle);
        let Some(operation) = self.operations.get_mut(&handle) else {
            return;
        };
        let completed = match (operation, response) {
            (Operation::Host(state), Response::Host(response)) => {
                *state = State::Ready(response);
                true
            }
            (Operation::Http { state, .. }, Response::Http(response)) => {
                *state = State::Ready(response);
                true
            }
            (Operation::HttpStreamRead(state), Response::HttpStream(response)) => {
                *state = State::Ready(response);
                true
            }
            (Operation::HttpUploadWrite(state), Response::HttpUploadWrite(response)) => {
                *state = State::Ready(response);
                true
            }
            (
                Operation::Host(_),
                Response::Http(_) | Response::HttpStream(_) | Response::HttpUploadWrite(_),
            )
            | (Operation::Http { .. }, Response::Host(_) | Response::HttpUploadWrite(_))
            | (Operation::Http { .. }, Response::HttpStream(_))
            | (
                Operation::Sleep(_),
                Response::Host(_)
                | Response::Http(_)
                | Response::HttpStream(_)
                | Response::HttpUploadWrite(_),
            )
            | (
                Operation::HttpStreamRead(_),
                Response::Host(_) | Response::Http(_) | Response::HttpUploadWrite(_),
            )
            | (
                Operation::HttpUploadWrite(_),
                Response::Host(_) | Response::Http(_) | Response::HttpStream(_),
            ) => false,
        };
        if completed {
            self.mark_ready(handle);
        }
    }

    fn get(&self, handle: u32) -> Option<&Operation> {
        self.operations.get(&handle)
    }

    fn remove(&mut self, handle: u32) -> Result<Operation, InvalidHandle> {
        let operation = self
            .operations
            .remove(&handle)
            .ok_or(InvalidHandle(handle))?;
        if let Some(abort_handle) = self.abort_handles.remove(&handle) {
            abort_handle.abort();
        }
        if let Some(index) = self.ready_indexes.remove(&handle) {
            let moved = *self.ready.last().expect("ready index points to an entry");
            self.ready.swap_remove(index);
            if moved != handle {
                self.ready_indexes.insert(moved, index);
            }
        }
        Ok(operation)
    }

    fn release(&mut self, handle: u32) {
        let _ = self.remove(handle);
    }

    fn clear(&mut self) {
        for (_, abort_handle) in self.abort_handles.drain() {
            abort_handle.abort();
        }
        self.operations.clear();
        self.in_flight = InFlightSet::new();
        self.deferred.clear();
        self.ready.clear();
        self.ready_indexes.clear();
        self.deadlines.clear();
        self.uploads.clear();
    }

    fn clear_streams(&mut self) {
        self.streams.clear();
    }
}

thread_local! {
    static OPERATIONS: RefCell<Registry> = RefCell::new(Registry::new());
}

fn register(operation: Operation) -> u32 {
    OPERATIONS.with(|operations| operations.borrow_mut().insert(operation))
}

/// Register a deferred hostcall.
#[must_use]
pub fn register_hostcall(call_type: String, payload: Vec<u8>) -> u32 {
    register(Operation::Host(State::Deferred(HostRequest {
        call_type,
        payload,
    })))
}

/// Register a deferred request sent through `wasi:http/client`.
#[must_use]
pub fn register_http(request: HttpRequest) -> u32 {
    let request_url = request.url().to_string();
    register(Operation::Http {
        request_url,
        state: State::Deferred(request),
    })
}

/// Register one asynchronous read from a native response stream.
#[must_use]
pub fn register_http_stream_read(handle: u32) -> u32 {
    register(Operation::HttpStreamRead(State::Deferred(handle)))
}

/// Register a response body stream and return its native handle.
#[must_use]
pub fn register_http_stream(source: HttpBodyStream) -> u32 {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let handle = operations.allocate_handle();
        operations.streams.insert(handle, HttpStream::new(source));
        handle
    })
}

/// Create a bounded producer/consumer pair for an HTTP request body.
#[must_use]
pub fn register_http_upload(capacity: usize) -> u32 {
    let capacity = capacity.max(1);
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let handle = operations.allocate_handle();
        let (sender, receiver) = mpsc::channel(capacity);
        operations.uploads.insert(
            handle,
            HttpUpload {
                sender: Rc::new(Mutex::new(sender)),
                receiver: Some(receiver),
            },
        );
        handle
    })
}

/// Take the consumer side of a registered HTTP upload.
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the upload is unknown or already attached to
/// a request.
pub fn take_http_upload_stream(handle: u32) -> Result<HttpBodyStream, InvalidHandle> {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let upload = operations
            .uploads
            .get_mut(&handle)
            .ok_or(InvalidHandle(handle))?;
        let receiver = upload.receiver.take().ok_or(InvalidHandle(handle))?;
        Ok(Box::pin(receiver) as HttpBodyStream)
    })
}

/// Queue one request-body chunk and return a pollable operation handle.
///
/// The operation becomes ready only when the bounded upload channel accepts
/// the chunk, which provides backpressure to language runtimes.
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the upload is unknown or closed.
pub fn register_http_upload_write(
    handle: u32,
    chunk: Result<Vec<u8>, String>,
) -> Result<u32, InvalidHandle> {
    let sender = OPERATIONS.with(|operations| {
        operations
            .borrow()
            .uploads
            .get(&handle)
            .map(|upload| upload.sender.clone())
            .ok_or(InvalidHandle(handle))
    })?;
    let mut channel = sender.lock();
    match channel.try_send(chunk) {
        Ok(()) => Ok(register(Operation::HttpUploadWrite(State::Ready(Ok(()))))),
        Err(error) if error.is_full() => Ok(register(Operation::HttpUploadWrite(State::Deferred(
            HttpUploadWrite {
                sender: sender.clone(),
                chunk: error.into_inner(),
            },
        )))),
        // The only remaining `try_send` failure mode is a disconnected channel.
        Err(_) => Ok(register(Operation::HttpUploadWrite(State::Ready(Err(
            UPLOAD_CLOSED.to_string(),
        ))))),
    }
}

/// Close an upload producer. Buffered chunks remain available to the request.
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the upload is unknown or already closed.
pub fn close_http_upload(handle: u32) -> Result<(), InvalidHandle> {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let upload = operations
            .uploads
            .remove(&handle)
            .ok_or(InvalidHandle(handle))?;
        upload.sender.lock().close_channel();
        Ok(())
    })
}

/// Read an already-ready chunk without entering the component async executor.
/// Production consumers should use [`read_http_stream_async`].
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the stream handle is unknown or released.
pub fn read_http_stream(handle: u32) -> Result<Option<Vec<u8>>, InvalidHandle> {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let stream = operations
            .streams
            .get_mut(&handle)
            .ok_or(InvalidHandle(handle))?;
        if stream.cancelled {
            return Ok(None);
        }
        let mut source = std::mem::replace(&mut stream.source, Box::pin(futures::stream::empty()));
        let mut cx = Context::from_waker(Waker::noop());
        let value = match source.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(Ok(chunk))) => Some(chunk),
            Poll::Ready(Some(Err(_)) | None) | Poll::Pending => None,
        };
        stream.source = source;
        Ok(value)
    })
}

/// Read one chunk from a lazily-produced response body stream.
///
/// # Errors
///
/// Returns an error when the stream handle is unknown or the source fails.
#[expect(
    clippy::future_not_send,
    reason = "response streams and the pending registry are intentionally local"
)]
pub async fn read_http_stream_async(handle: u32) -> Result<Option<Vec<u8>>, String> {
    let Some(mut source_stream) = OPERATIONS.with(|operations| -> Result<_, String> {
        let mut operations = operations.borrow_mut();
        let stream = operations
            .streams
            .get_mut(&handle)
            .ok_or_else(|| InvalidHandle(handle).to_string())?;
        if stream.cancelled {
            return Ok(None);
        }
        Ok(Some(std::mem::replace(
            &mut stream.source,
            Box::pin(futures::stream::empty()),
        )))
    })?
    else {
        return Ok(None);
    };
    let next = source_stream.next().await;
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let Some(stream) = operations.streams.get_mut(&handle) else {
            return;
        };
        if stream.cancelled {
            return;
        }
        if next.is_some() && !matches!(next, Some(Err(_))) {
            stream.source = source_stream;
        }
    });
    match next {
        None => Ok(None),
        Some(Ok(chunk)) => Ok(Some(chunk)),
        Some(Err(error)) => Err(error),
    }
}

/// Cancel a response body stream and discard buffered chunks.
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the stream handle is unknown or released.
pub fn cancel_http_stream(handle: u32) -> Result<(), InvalidHandle> {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        let stream = operations
            .streams
            .get_mut(&handle)
            .ok_or(InvalidHandle(handle))?;
        stream.cancelled = true;
        stream.source = Box::pin(futures::stream::empty());
        Ok(())
    })
}

/// Release a response body stream handle.
///
/// # Errors
///
/// Returns [`InvalidHandle`] when the stream handle is unknown or already
/// released.
pub fn release_http_stream(handle: u32) -> Result<(), InvalidHandle> {
    OPERATIONS.with(|operations| {
        operations
            .borrow_mut()
            .streams
            .remove(&handle)
            .map(|_| ())
            .ok_or(InvalidHandle(handle))
    })
}

/// Register a sleep deadline.
#[must_use]
pub fn register_sleep(deadline: Deadline) -> u32 {
    register(Operation::Sleep(deadline))
}

/// Return all handles whose operation can be consumed without blocking.
#[must_use]
pub fn ready_handles() -> Vec<u32> {
    let now = Instant::now();
    OPERATIONS.with(|operations| operations.borrow_mut().ready_handles(now))
}

/// Return whether an operation is ready or no longer registered.
///
/// Unknown handles are treated as ready so callers can consume the handle and
/// receive its concrete invalid-handle error instead of blocking forever.
#[must_use]
pub fn is_ready(handle: u32) -> bool {
    let now = Instant::now();
    OPERATIONS.with(|operations| {
        operations
            .borrow()
            .get(handle)
            .is_none_or(|operation| operation.is_ready(now))
    })
}

/// Return whether a handle represents a response-body stream read.
#[must_use]
pub fn is_http_stream_read(handle: u32) -> bool {
    OPERATIONS.with(|operations| {
        matches!(
            operations.borrow().get(handle),
            Some(Operation::HttpStreamRead(_))
        )
    })
}

/// Return whether the registry contains any operation.
#[must_use]
pub fn has_pending() -> bool {
    OPERATIONS.with(|operations| !operations.borrow().operations.is_empty())
}

/// Control how the pending-operation driver advances after a language-runtime
/// callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Drive {
    /// Stop the driver and cancel every unfinished operation.
    Stop,
    /// Stop the driver while preserving unfinished operations for re-entry.
    Suspend,
    /// Wait until an operation or deadline completes.
    Wait,
}

/// Drive pending operations until `step` requests a stop.
///
/// When `step` returns [`Drive::Suspend`], unfinished imported futures remain
/// alive across calls so async generators can yield without abandoning work
/// that spans generator turns. `step` runs before every wait so language
/// runtimes can consume ready values, register follow-up work, or cancel
/// losers.
#[must_use]
pub fn drive_pending(mut step: impl FnMut() -> Drive) -> bool {
    block_on(async {
        let mut in_flight = OPERATIONS.with(|operations| operations.borrow_mut().take_in_flight());
        let mut made_progress = false;

        loop {
            let ready_generation = OPERATIONS.with(|operations| {
                let mut operations = operations.borrow_mut();
                operations.refresh_ready(Instant::now());
                operations.ready_generation
            });
            let mode = step();
            match mode {
                Drive::Stop => return made_progress,
                Drive::Suspend => {
                    OPERATIONS.with(|operations| {
                        operations.borrow_mut().suspend(in_flight);
                    });
                    return made_progress;
                }
                Drive::Wait => {}
            }

            let new_ready = OPERATIONS.with(|operations| {
                let mut operations = operations.borrow_mut();
                operations.start_deferred(&in_flight);
                operations.refresh_ready(Instant::now());
                operations.ready_generation != ready_generation
            });

            if new_ready {
                made_progress = true;
                made_progress |= drain_completed(&mut in_flight);
                let has_running =
                    OPERATIONS.with(|operations| !operations.borrow().abort_handles.is_empty());
                if has_running
                    && let Some(completion) = wait_for_completion(
                        &mut in_flight,
                        Some(Instant::now() + FAIRNESS_YIELD),
                        false,
                    )
                    .await
                {
                    made_progress |= complete(completion);
                    made_progress |= drain_completed(&mut in_flight);
                }
                continue;
            }

            let (deadline, has_never_deadline) = OPERATIONS.with(|operations| {
                let mut operations = operations.borrow_mut();
                (
                    operations.next_deadline(Instant::now()),
                    operations.has_never_deadline(),
                )
            });
            if in_flight.is_empty() && deadline.is_none() && !has_never_deadline {
                return made_progress;
            }

            let Some(completion) =
                wait_for_completion(&mut in_flight, deadline, has_never_deadline).await
            else {
                return made_progress;
            };
            made_progress |= complete(completion);
            made_progress |= drain_completed(&mut in_flight);
        }
    })
}

fn drain_completed(in_flight: &mut InFlightSet) -> bool {
    let mut made_progress = false;
    while let Some(Some(completion)) = in_flight.next().now_or_never() {
        made_progress |= complete(completion);
    }
    made_progress
}

#[expect(
    clippy::future_not_send,
    reason = "guest imports and their thread-local registry are intentionally non-Send"
)]
async fn wait_for_completion(
    in_flight: &mut InFlightSet,
    deadline: Option<Instant>,
    wait_forever: bool,
) -> Option<Completion> {
    loop {
        let completion = if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let remaining_nanos = u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX);
            if in_flight.is_empty() {
                monotonic_clock::wait_for(remaining_nanos).await;
                return Some(Completion::Deadline);
            }

            let operation = in_flight.next();
            let timer = monotonic_clock::wait_for(remaining_nanos);
            pin_mut!(operation, timer);
            match select(operation, timer).await {
                Either::Left((completion, _timer)) => completion?,
                Either::Right(((), _operation)) => return Some(Completion::Deadline),
            }
        } else if in_flight.is_empty() && wait_forever {
            monotonic_clock::wait_for(u64::MAX).await;
            return Some(Completion::Deadline);
        } else {
            in_flight.next().await?
        };
        match completion {
            Completion::Operation(_, None) => {}
            Completion::Operation(_, Some(_)) | Completion::Deadline => return Some(completion),
        }
    }
}

fn complete(completion: Completion) -> bool {
    match completion {
        Completion::Operation(handle, Some(response)) => {
            OPERATIONS.with(|operations| operations.borrow_mut().complete(handle, response));
            true
        }
        Completion::Deadline => true,
        Completion::Operation(_, None) => false,
    }
}

fn take_operation(handle: u32) -> Result<Operation, InvalidHandle> {
    OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        operations.remove(handle)
    })
}

/// Remove an operation and return its completed output, if ready.
///
/// An undriven or running operation is removed and reported as pending.
///
/// # Errors
///
/// Returns [`InvalidHandle`] if the handle is unknown or was consumed.
pub fn take(handle: u32) -> Result<Take, InvalidHandle> {
    Ok(match take_operation(handle)? {
        Operation::Host(State::Ready(result)) => Take::Ready(Output::Host(result)),
        Operation::Http {
            request_url,
            state: State::Ready(response),
        } => Take::Ready(Output::Http {
            request_url,
            response,
        }),
        Operation::Sleep(deadline) if deadline.is_ready() => Take::Ready(Output::Sleep),
        Operation::HttpStreamRead(State::Ready(result)) => Take::Ready(Output::HttpStream(result)),
        Operation::HttpUploadWrite(State::Ready(result)) => {
            Take::Ready(Output::HttpUploadWrite(result))
        }
        Operation::Host(State::Deferred(_) | State::Running)
        | Operation::Http {
            state: State::Deferred(_) | State::Running,
            ..
        }
        | Operation::Sleep(_)
        | Operation::HttpStreamRead(State::Deferred(_) | State::Running)
        | Operation::HttpUploadWrite(State::Deferred(_) | State::Running) => Take::Pending,
    })
}

/// Remove one operation, driving it synchronously when necessary.
///
/// # Errors
///
/// Returns [`InvalidHandle`] if the handle is unknown, consumed, or already
/// being driven.
pub fn drive_one(handle: u32) -> Result<Output, InvalidHandle> {
    match take_operation(handle)? {
        Operation::Host(State::Ready(result)) => Ok(Output::Host(result)),
        Operation::Host(State::Deferred(HostRequest { call_type, payload })) => {
            Ok(Output::Host(block_on(host::hostcall(call_type, payload))))
        }
        Operation::Http {
            request_url,
            state: State::Ready(response),
        } => Ok(Output::Http {
            request_url,
            response,
        }),
        Operation::HttpStreamRead(State::Ready(result)) => Ok(Output::HttpStream(result)),
        Operation::HttpStreamRead(State::Deferred(handle)) => {
            Ok(Output::HttpStream(block_on(read_http_stream_async(handle))))
        }
        Operation::HttpUploadWrite(State::Ready(result)) => Ok(Output::HttpUploadWrite(result)),
        Operation::HttpUploadWrite(State::Deferred(HttpUploadWrite { sender, chunk })) => {
            Ok(Output::HttpUploadWrite(block_on(async move {
                let mut sender = sender.lock().clone();
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| UPLOAD_CLOSED.to_string())
            })))
        }
        Operation::Http {
            request_url,
            state: State::Deferred(request),
        } => Ok(Output::Http {
            request_url,
            response: block_on(wasi_http::send(request)),
        }),
        Operation::Sleep(deadline) => {
            deadline.wait();
            Ok(Output::Sleep)
        }
        Operation::Host(State::Running)
        | Operation::Http {
            state: State::Running,
            ..
        }
        | Operation::HttpStreamRead(State::Running)
        | Operation::HttpUploadWrite(State::Running) => Err(InvalidHandle(handle)),
    }
}

/// Remove an operation without consuming its output.
pub fn release(handle: u32) {
    OPERATIONS.with(|operations| operations.borrow_mut().release(handle));
}

/// Cancel and remove every registered operation without resetting handle IDs.
pub fn clear() {
    let _ = drive_pending(|| Drive::Stop);
    OPERATIONS.with(|operations| operations.borrow_mut().clear());
}

/// Release every registered response body stream.
///
/// Unlike [`clear`], this is intended for sandbox/instance teardown rather
/// than ordinary call-boundary cleanup. Response streams can legitimately be
/// retained across calls, so clearing them after every guest invocation would
/// invalidate those responses.
pub fn clear_http_streams() {
    OPERATIONS.with(|operations| operations.borrow_mut().clear_streams());
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Output, Take};
    use crate::Deadline;

    #[test]
    fn storage_is_freed_without_reusing_handles() {
        let released = super::register_sleep(Deadline::default());
        super::release(released);

        let replacement = super::register_sleep(Deadline::default());
        assert_ne!(replacement, released);
        assert_eq!(
            super::OPERATIONS.with(|operations| operations.borrow().operations.len()),
            1
        );

        super::release(released);
        assert!(matches!(
            super::take(replacement),
            Ok(Take::Ready(Output::Sleep))
        ));
    }

    #[test]
    fn consumed_handles_are_invalid() {
        let handle = super::register_sleep(Deadline::default());
        assert!(matches!(
            super::take(handle),
            Ok(Take::Ready(Output::Sleep))
        ));
        let Err(error) = super::take(handle) else {
            panic!("consumed handle should be rejected");
        };
        assert_eq!(error.handle(), handle);
    }

    #[test]
    fn future_sleep_is_not_ready_when_taken_early() {
        let deadline = Deadline::after(Duration::from_secs(60)).unwrap();
        let handle = super::register_sleep(deadline);
        assert!(!super::ready_handles().contains(&handle));
        assert!(matches!(super::take(handle), Ok(Take::Pending)));
    }

    #[test]
    fn next_deadline_ignores_ready_sleep() {
        let ready = super::register_sleep(Deadline::default());
        let future = Deadline::after(Duration::from_secs(60)).unwrap();
        let waiting = super::register_sleep(future);

        let next = super::OPERATIONS
            .with(|operations| operations.borrow_mut().next_deadline(Instant::now()));
        assert_eq!(next, future.ready_at());

        super::release(ready);
        super::release(waiting);
    }

    #[test]
    fn immediate_sleeps_are_ready_in_registration_order() {
        let first = super::register_sleep(Deadline::default());
        let second = super::register_sleep(Deadline::default());
        let third = super::register_sleep(Deadline::default());

        assert_eq!(super::ready_handles(), vec![first, second, third]);

        super::release(first);
        super::release(second);
        super::release(third);
    }

    #[test]
    fn immediately_accepted_upload_writes_are_ready() {
        let upload = super::register_http_upload(1);
        let write = super::register_http_upload_write(upload, Ok(vec![1])).unwrap();

        assert!(super::ready_handles().contains(&write));
        assert!(matches!(
            super::take(write),
            Ok(Take::Ready(Output::HttpUploadWrite(Ok(()))))
        ));

        super::close_http_upload(upload).unwrap();
    }
}
