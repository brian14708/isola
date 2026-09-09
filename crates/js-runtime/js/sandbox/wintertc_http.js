"use strict";

(function () {
  var hasOwn = Object.prototype.hasOwnProperty;

  // QuickJS does not provide Web Streams. Keep the guest-facing contract
  // available with the small subset needed by fetch response bodies.
  if (typeof globalThis.ReadableStream === "undefined") {
    class IsolaReadableStream {
      constructor(source) {
        this._source = source || {};
        this._queue = [];
        this._waiters = [];
        this._closed = false;
        this._error = null;
        this._pulling = false;
        this._cancelled = false;
        this._started = false;
        this._startError = null;
        if (typeof this._source.start === "function") {
          try {
            var result = this._source.start(this._controller());
            if (result && typeof result.then === "function") {
              result.catch((error) => {
                this._startError = error;
                this._error = error;
              });
            }
          } catch (error) {
            this._startError = error;
            this._error = error;
          }
        }
      }

      getReader() {
        if (this._locked) throw new TypeError("ReadableStream is locked");
        this._locked = true;
        var stream = this;
        return {
          read: function () {
            return stream._read();
          },
          cancel: function (reason) {
            return stream._cancel(reason);
          },
          releaseLock: function () {
            stream._locked = false;
          },
        };
      }

      _read() {
        if (this._queue.length > 0) {
          return Promise.resolve({ value: this._queue.shift(), done: false });
        }
        if (this._error !== null) return Promise.reject(this._error);
        if (this._closed) return Promise.resolve({ value: undefined, done: true });
        var stream = this;
        var promise = new Promise(function (resolve, reject) {
          stream._waiters.push({ resolve: resolve, reject: reject });
        });
        this._pump();
        return promise;
      }

      _controller() {
        var stream = this;
        return {
          enqueue: function (value) {
            if (stream._closed || stream._cancelled) return;
            var waiter = stream._waiters.shift();
            if (waiter) waiter.resolve({ value: value, done: false });
            else stream._queue.push(value);
          },
          close: function () {
            stream._closed = true;
            var waiter;
            while ((waiter = stream._waiters.shift())) waiter.resolve({ value: undefined, done: true });
          },
          error: function (error) {
            stream._error = error;
            var waiter;
            while ((waiter = stream._waiters.shift())) waiter.reject(error);
          },
        };
      }

      _pump() {
        if (this._pulling || this._closed || this._cancelled) return;
        this._pulling = true;
        var stream = this;
        var controller = stream._controller();
        Promise.resolve()
          .then(function () {
            if (typeof stream._source.pull === "function") {
              return stream._source.pull(controller);
            }
            controller.close();
            return undefined;
          })
          .catch(controller.error)
          .then(function () {
            stream._pulling = false;
            if (stream._waiters.length > 0) stream._pump();
          });
      }

      _cancel(reason) {
        this._cancelled = true;
        this._closed = true;
        this._queue = [];
        var waiter;
        while ((waiter = this._waiters.shift())) {
          waiter.resolve({ value: undefined, done: true });
        }
        if (typeof this._source.cancel === "function") {
          return Promise.resolve(this._source.cancel(reason));
        }
        return Promise.resolve();
      }
    }
    globalThis.ReadableStream = IsolaReadableStream;
  }

  function abortError(reason) {
    if (typeof globalThis.__isolaAbortError === "function") {
      return globalThis.__isolaAbortError(reason);
    }
    var message =
      reason === undefined ? "The operation was aborted." : String(reason);
    var error = new Error(message);
    error.name = "AbortError";
    return error;
  }

  function isArrayBuffer(value) {
    return typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer;
  }

  function isArrayBufferView(value) {
    return (
      typeof ArrayBuffer !== "undefined" &&
      typeof ArrayBuffer.isView === "function" &&
      ArrayBuffer.isView(value)
    );
  }

  function copyArrayBuffer(buffer) {
    if (!isArrayBuffer(buffer)) {
      return null;
    }
    return buffer.slice(0);
  }

  function copyViewToArrayBuffer(view) {
    var bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
    var copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    return copy.buffer;
  }

  function isAsyncIterable(value) {
    return (
      value !== null &&
      value !== undefined &&
      typeof Symbol !== "undefined" &&
      Symbol.asyncIterator !== undefined &&
      typeof value[Symbol.asyncIterator] === "function"
    );
  }

  function encodeUtf8(input) {
    var text = String(input);
    if (typeof TextEncoder !== "undefined") {
      return new TextEncoder().encode(text).buffer;
    }

    var encoded = encodeURIComponent(text);
    var bytes = [];
    for (var i = 0; i < encoded.length; i += 1) {
      if (encoded[i] === "%" && i + 2 < encoded.length) {
        bytes.push(parseInt(encoded.slice(i + 1, i + 3), 16));
        i += 2;
      } else {
        bytes.push(encoded.charCodeAt(i));
      }
    }
    return Uint8Array.from(bytes).buffer;
  }

  function decodeUtf8(bytes) {
    if (typeof TextDecoder !== "undefined") {
      return new TextDecoder().decode(bytes);
    }

    var encoded = "";
    for (var i = 0; i < bytes.length; i += 1) {
      var hex = bytes[i].toString(16).toUpperCase();
      if (hex.length < 2) {
        hex = "0" + hex;
      }
      encoded += "%" + hex;
    }

    try {
      return decodeURIComponent(encoded);
    } catch (_err) {
      var fallback = "";
      for (var j = 0; j < bytes.length; j += 1) {
        fallback += String.fromCharCode(bytes[j]);
      }
      return fallback;
    }
  }

  function normalizeMethod(value) {
    return String(value || "GET").toUpperCase();
  }

  function normalizeUrl(input) {
    if (typeof input === "string") {
      return input;
    }
    if (typeof URL !== "undefined" && input instanceof URL) {
      return input.toString();
    }
    if (
      input !== null &&
      input !== undefined &&
      typeof input.toString === "function"
    ) {
      return input.toString();
    }
    throw new TypeError("Failed to construct Request: invalid URL input");
  }

  function normalizeHeaderName(name) {
    var normalized = String(name).toLowerCase();
    if (normalized.length === 0) {
      throw new TypeError("Header name cannot be empty.");
    }
    return normalized;
  }

  function normalizeHeaderValue(value) {
    return String(value);
  }

  function cloneHeaderList(list) {
    var out = [];
    for (var i = 0; i < list.length; i += 1) {
      out.push([list[i][0], list[i][1]]);
    }
    return out;
  }

  function toBodyBytes(body) {
    if (body === null || body === undefined) {
      return null;
    }
    if (isArrayBuffer(body)) {
      return copyArrayBuffer(body);
    }
    if (isArrayBufferView(body)) {
      return copyViewToArrayBuffer(body);
    }
    return null;
  }

  function setDefaultContentType(headers, value) {
    if (!headers.has("content-type")) {
      headers.set("content-type", value);
    }
  }

  function normalizeBody(body, headers, forRequest) {
    if (body === undefined || body === null) {
      return { bytes: null, text: null, stream: null };
    }

    if (
      forRequest &&
      ((typeof ReadableStream !== "undefined" && body instanceof ReadableStream) ||
        isAsyncIterable(body))
    ) {
      return { bytes: null, text: null, stream: body };
    }

    if (isArrayBuffer(body)) {
      return { bytes: copyArrayBuffer(body), text: null, stream: null };
    }

    if (isArrayBufferView(body)) {
      return { bytes: copyViewToArrayBuffer(body), text: null, stream: null };
    }

    if (
      typeof URLSearchParams !== "undefined" &&
      body instanceof URLSearchParams
    ) {
      var formText = body.toString();
      if (forRequest) {
        setDefaultContentType(
          headers,
          "application/x-www-form-urlencoded;charset=UTF-8",
        );
      }
      return { bytes: encodeUtf8(formText), text: formText, stream: null };
    }

    if (typeof body === "string") {
      if (forRequest) {
        setDefaultContentType(headers, "text/plain;charset=UTF-8");
      }
      return { bytes: encodeUtf8(body), text: body, stream: null };
    }

    if (forRequest && typeof body === "object") {
      var jsonText = JSON.stringify(body);
      if (jsonText === undefined) {
        jsonText = "null";
      }
      setDefaultContentType(headers, "application/json");
      return { bytes: encodeUtf8(jsonText), text: jsonText, stream: null };
    }

    var fallbackText = String(body);
    if (forRequest) {
      setDefaultContentType(headers, "text/plain;charset=UTF-8");
    }
    return { bytes: encodeUtf8(fallbackText), text: fallbackText, stream: null };
  }

  function consumeBody(instance) {
    if (instance.bodyUsed) {
      return Promise.reject(new TypeError("Body has already been consumed."));
    }

    instance.bodyUsed = true;
    if (instance.body !== null && instance.body !== undefined) {
      return consumeReadableStream(instance);
    }
    // Keep the native fallback for environments where a response stream could
    // not be exposed, while normal responses always consume through body.
    if (instance._streamHandle !== null && instance._streamHandle !== undefined) {
      return consumeStream(instance);
    }
    if (instance._bodyBytes === null) {
      return Promise.resolve(new ArrayBuffer(0));
    }
    return Promise.resolve(copyArrayBuffer(instance._bodyBytes));
  }

  function textBody(instance) {
    return consumeBody(instance).then(function (bytes) {
      if (
        (instance._streamHandle === null || instance._streamHandle === undefined) &&
        instance._bodyText !== null &&
        instance._bodyText !== undefined
      ) {
        return instance._bodyText;
      }
      var text = decodeUtf8(new Uint8Array(bytes));
      instance._bodyText = text;
      return text;
    });
  }

  function jsonBody(instance) {
    return textBody(instance).then(function (text) {
      return JSON.parse(text);
    });
  }

  function arrayBufferBody(instance) {
    return consumeBody(instance);
  }

  function releaseStream(instance) {
    var handle = instance._streamHandle;
    if (handle === null || handle === undefined || instance._streamReleased) {
      return;
    }
    instance._streamReleased = true;
    detachAbortSignal(instance);
    if (globalThis._isola_http && typeof globalThis._isola_http._release === "function") {
      try {
        globalThis._isola_http._release(handle);
      } catch (_err) {
        // Releasing an already-finished native stream is idempotent.
      }
    }
  }

  function detachAbortSignal(instance) {
    if (
      instance._abortSignal !== null &&
      instance._abortSignal !== undefined &&
      instance._abortHandler !== null &&
      instance._abortHandler !== undefined
    ) {
      instance._abortSignal.removeEventListener("abort", instance._abortHandler);
    }
    instance._abortSignal = null;
    instance._abortHandler = null;
    instance._abortResolve = null;
    instance._abortPromise = null;
  }

  function attachAbortSignal(instance, signal) {
    if (
      (signal === null || signal === undefined) ||
      (instance._streamHandle === null || instance._streamHandle === undefined)
    ) {
      return;
    }

    instance._abortSignal = signal;
    instance._abortPromise = new Promise(function (resolve) {
      instance._abortResolve = resolve;
    });
    instance._abortHandler = function () {
      if (instance._abortError !== null && instance._abortError !== undefined) {
        return;
      }
      var error = abortError(signal.reason);
      instance._abortError = error;
      if (instance._abortResolve !== null && instance._abortResolve !== undefined) {
        instance._abortResolve(error);
      }
      // Cancel the native body as soon as the signal fires. Pending reads also
      // race _abortPromise and cancel their individual pollable handles.
      void cancelStream(instance, error).catch(function () {});
    };
    signal.addEventListener("abort", instance._abortHandler);
    if (signal.aborted) {
      instance._abortHandler();
    }
  }

  function cancelStream(instance, reason) {
    var handle = instance._streamHandle;
    if (
      handle === null ||
      handle === undefined ||
      instance._streamCancelled ||
      instance._streamReleased
    ) {
      return Promise.resolve();
    }
    instance._streamCancelled = true;
    if (
      instance._readHandle !== null &&
      instance._readHandle !== undefined &&
      typeof _isola_async !== "undefined" &&
      typeof _isola_async._cancel === "function"
    ) {
      _isola_async._cancel(instance._readHandle, reason);
      instance._readHandle = null;
    }
    var cancel = globalThis._isola_http && globalThis._isola_http._cancel;
    if (typeof cancel !== "function") {
      releaseStream(instance);
      return Promise.resolve();
    }
    try {
      var result = cancel(handle, reason === undefined ? null : String(reason));
      return Promise.resolve(result).then(
        function () {
          releaseStream(instance);
        },
        function (error) {
          releaseStream(instance);
          throw error;
        },
      );
    } catch (error) {
      releaseStream(instance);
      return Promise.reject(error);
    }
  }

  function readStreamChunk(instance) {
    if (instance._abortError !== null && instance._abortError !== undefined) {
      return Promise.reject(instance._abortError);
    }
    var read = globalThis._isola_http && globalThis._isola_http._read;
    var finish = globalThis._isola_http && globalThis._isola_http._finishRead;
    if (typeof read !== "function") {
      return Promise.reject(new TypeError("HTTP response stream is unavailable."));
    }
    try {
      var handle = read(instance._streamHandle);
      instance._readHandle = handle;
      if (typeof finish !== "function" || typeof _isola_async === "undefined") {
        var unavailable = new TypeError("HTTP response stream completion is unavailable.");
        if (typeof _isola_async !== "undefined" && typeof _isola_async._cancel === "function") {
          _isola_async._cancel(handle, unavailable);
        }
        instance._readHandle = null;
        return Promise.reject(unavailable);
      }
      var wait = _isola_async._wait(handle, function () {
        return finish(handle);
      }).then(
        function (result) {
          if (instance._readHandle === handle) instance._readHandle = null;
          return result;
        },
        function (error) {
          if (instance._readHandle === handle) instance._readHandle = null;
          throw error;
        },
      );
      if (instance._abortPromise === null || instance._abortPromise === undefined) {
        return wait;
      }
      return Promise.race([
        wait,
        instance._abortPromise.then(function (error) {
          if (typeof _isola_async._cancel === "function") {
            _isola_async._cancel(handle, error);
          }
          throw error;
        }),
      ]);
    } catch (error) {
      return Promise.reject(error);
    }
  }

  function chunkToBytes(chunk) {
    if (chunk === null || chunk === undefined) return new Uint8Array(0);
    if (chunk instanceof Uint8Array) return chunk;
    if (isArrayBuffer(chunk)) return new Uint8Array(chunk);
    if (isArrayBufferView(chunk)) {
      return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    }
    if (chunk.value !== undefined) return chunkToBytes(chunk.value);
    return new Uint8Array(0);
  }

  function consumeStream(instance) {
    var parts = [];
    var total = 0;
    function next() {
      return readStreamChunk(instance).then(function (result) {
        var done = result === null || result === undefined || result.done === true;
        if (done) {
          releaseStream(instance);
          var output = new Uint8Array(total);
          var offset = 0;
          for (var i = 0; i < parts.length; i += 1) {
            output.set(parts[i], offset);
            offset += parts[i].length;
          }
          return output.buffer;
        }
        var bytes = chunkToBytes(result);
        if (bytes.length > 0) {
          parts.push(new Uint8Array(bytes));
          total += bytes.length;
        }
        return next();
      });
    }
    return next().catch(function (error) {
      releaseStream(instance);
      throw error;
    });
  }

  function consumeReadableStream(instance) {
    var reader;
    try {
      reader = instance.body.getReader();
    } catch (error) {
      return Promise.reject(error);
    }
    var released = false;
    var parts = [];
    var total = 0;
    function releaseReader() {
      if (released) return;
      released = true;
      try {
        reader.releaseLock();
      } catch (_err) {
        // The stream may already have released the reader after an error.
      }
    }
    function next() {
      return reader.read().then(function (result) {
        if (result === null || result === undefined || result.done === true) {
          releaseReader();
          var output = new Uint8Array(total);
          var offset = 0;
          for (var i = 0; i < parts.length; i += 1) {
            output.set(parts[i], offset);
            offset += parts[i].length;
          }
          return output.buffer;
        }
        var bytes = chunkToBytes(result.value);
        if (bytes.length > 0) {
          parts.push(new Uint8Array(bytes));
          total += bytes.length;
        }
        return next();
      });
    }
    return next().catch(function (error) {
      releaseReader();
      throw error;
    });
  }

  function createResponseTee(instance) {
    var state = {
      queues: [],
      waiters: [],
      pulling: false,
      done: false,
      error: null,
      cancelled: [],
    };

    function settleWaiters() {
      for (var i = 0; i < state.waiters.length; i += 1) {
        var waiter;
        while ((waiter = state.waiters[i].shift())) {
          if (state.error !== null) waiter.reject(state.error);
          else waiter.resolve(null);
        }
      }
    }

    function pump() {
      if (state.pulling || state.done || state.error !== null) return;
      state.pulling = true;
      readStreamChunk(instance)
        .then(function (result) {
          var done = result === null || result === undefined || result.done === true;
          if (done) {
            state.done = true;
            releaseStream(instance);
            settleWaiters();
            return;
          }
          var bytes = new Uint8Array(chunkToBytes(result));
          for (var i = 0; i < state.waiters.length; i += 1) {
            if (state.cancelled[i]) continue;
            var waiter = state.waiters[i].shift();
            if (waiter) waiter.resolve(bytes);
            else state.queues[i].push(bytes);
          }
        })
        .catch(function (error) {
          state.error = error;
          releaseStream(instance);
          settleWaiters();
        })
        .then(function () {
          state.pulling = false;
          if (state.waiters.some(function (waiters) {
            return waiters.length > 0;
          })) {
            pump();
          }
        });
    }

    state.read = function (index) {
      if (state.queues[index].length > 0) {
        return Promise.resolve(state.queues[index].shift());
      }
      if (state.error !== null) return Promise.reject(state.error);
      if (state.done || state.cancelled[index]) return Promise.resolve(null);
      var promise = new Promise(function (resolve, reject) {
        state.waiters[index].push({resolve: resolve, reject: reject});
      });
      pump();
      return promise;
    };

    state.cancel = function (index, reason) {
      if (state.cancelled[index]) return Promise.resolve();
      state.cancelled[index] = true;
      state.queues[index] = [];
      while (state.waiters[index].length > 0) {
        state.waiters[index].shift().resolve(null);
      }
      if (state.cancelled.every(function (cancelled) {
        return cancelled;
      })) {
        state.done = true;
        return cancelStream(instance, reason);
      }
      return Promise.resolve();
    };

    function branch(index, owner) {
      var pulling = null;
      var stream = new ReadableStream({
        pull: function (controller) {
          if (pulling !== null) return pulling;
          pulling = state
            .read(index)
            .then(function (bytes) {
              if (bytes === null) controller.close();
              else controller.enqueue(new Uint8Array(bytes));
            })
            .finally(function () {
              pulling = null;
            });
          return pulling;
        },
        cancel: function (reason) {
          return state.cancel(index, reason);
        },
      });
      var getReader = stream.getReader;
      stream.getReader = function () {
        owner.bodyUsed = true;
        return getReader.call(stream);
      };
      return stream;
    }

    function addBranch(owner) {
      var index = state.queues.length;
      state.queues.push([]);
      state.waiters.push([]);
      state.cancelled.push(false);
      return branch(index, owner);
    }

    return {addBranch: addBranch};
  }

  function makeResponseBody(instance) {
    if (typeof ReadableStream === "undefined") {
      return null;
    }
    if (instance._streamHandle === null || instance._streamHandle === undefined) {
      if (instance._bodyBytes === null) return null;
      var bytes = new Uint8Array(instance._bodyBytes);
      var offset = 0;
      var buffered = new ReadableStream({
        pull: function (controller) {
          instance.bodyUsed = true;
          if (offset >= bytes.length) {
            controller.close();
            return undefined;
          }
          var end = Math.min(offset + 65536, bytes.length);
          controller.enqueue(bytes.slice(offset, end));
          offset = end;
          return undefined;
        },
        cancel: function () {
          offset = bytes.length;
        },
      });
      var bufferedGetReader = buffered.getReader;
      buffered.getReader = function () {
        instance.bodyUsed = true;
        return bufferedGetReader.call(buffered);
      };
      return buffered;
    }
    var pulling = null;
    var stream = new ReadableStream({
      pull: function (controller) {
        instance.bodyUsed = true;
        if (pulling !== null) return pulling;
        pulling = readStreamChunk(instance)
          .then(function (result) {
            var done = result === null || result === undefined || result.done === true;
            if (done) {
              releaseStream(instance);
              controller.close();
            } else {
              var bytes = chunkToBytes(result);
              controller.enqueue(new Uint8Array(bytes));
            }
          })
          .catch(function (error) {
            releaseStream(instance);
            controller.error(error);
          })
          .then(function () {
            pulling = null;
          });
        return pulling;
      },
      cancel: function (reason) {
        return cancelStream(instance, reason);
      },
    });
    var getReader = stream.getReader;
    stream.getReader = function () {
      instance.bodyUsed = true;
      return getReader.call(stream);
    };
    return stream;
  }

  function encodeFormComponent(value) {
    return encodeURIComponent(value).replace(/%20/g, "+");
  }

  function decodeFormComponent(value) {
    return decodeURIComponent(value.replace(/\+/g, "%20"));
  }

  if (typeof URLSearchParams === "undefined") {
    class URLSearchParamsPolyfill {
      constructor(init) {
        this._pairs = [];

        if (init === undefined || init === null) {
          return;
        }

        if (typeof init === "string") {
          var query = init;
          if (query[0] === "?") {
            query = query.slice(1);
          }
          if (query.length === 0) {
            return;
          }

          var parts = query.split("&");
          for (var i = 0; i < parts.length; i += 1) {
            if (parts[i].length === 0) {
              continue;
            }
            var eqIndex = parts[i].indexOf("=");
            if (eqIndex < 0) {
              this.append(decodeFormComponent(parts[i]), "");
            } else {
              this.append(
                decodeFormComponent(parts[i].slice(0, eqIndex)),
                decodeFormComponent(parts[i].slice(eqIndex + 1)),
              );
            }
          }
          return;
        }

        if (init instanceof URLSearchParamsPolyfill) {
          this._pairs = init._pairs.slice();
          return;
        }

        if (typeof init[Symbol.iterator] === "function") {
          for (var pair of init) {
            if (!pair || pair.length < 2) {
              throw new TypeError("Invalid URLSearchParams initializer pair.");
            }
            this.append(pair[0], pair[1]);
          }
          return;
        }

        if (typeof init === "object") {
          var keys = Object.keys(init);
          for (var j = 0; j < keys.length; j += 1) {
            this.append(keys[j], init[keys[j]]);
          }
          return;
        }

        throw new TypeError("Unsupported URLSearchParams initializer.");
      }

      append(name, value) {
        this._pairs.push([String(name), String(value)]);
      }

      set(name, value) {
        name = String(name);
        value = String(value);
        this.delete(name);
        this._pairs.push([name, value]);
      }

      get(name) {
        name = String(name);
        for (var i = 0; i < this._pairs.length; i += 1) {
          if (this._pairs[i][0] === name) {
            return this._pairs[i][1];
          }
        }
        return null;
      }

      getAll(name) {
        name = String(name);
        var values = [];
        for (var i = 0; i < this._pairs.length; i += 1) {
          if (this._pairs[i][0] === name) {
            values.push(this._pairs[i][1]);
          }
        }
        return values;
      }

      has(name) {
        return this.get(name) !== null;
      }

      delete(name) {
        name = String(name);
        var next = [];
        for (var i = 0; i < this._pairs.length; i += 1) {
          if (this._pairs[i][0] !== name) {
            next.push(this._pairs[i]);
          }
        }
        this._pairs = next;
      }

      toString() {
        var out = [];
        for (var i = 0; i < this._pairs.length; i += 1) {
          out.push(
            encodeFormComponent(this._pairs[i][0]) +
              "=" +
              encodeFormComponent(this._pairs[i][1]),
          );
        }
        return out.join("&");
      }

      *entries() {
        for (var i = 0; i < this._pairs.length; i += 1) {
          yield [this._pairs[i][0], this._pairs[i][1]];
        }
      }

      *keys() {
        for (var i = 0; i < this._pairs.length; i += 1) {
          yield this._pairs[i][0];
        }
      }

      *values() {
        for (var i = 0; i < this._pairs.length; i += 1) {
          yield this._pairs[i][1];
        }
      }

      [Symbol.iterator]() {
        return this.entries();
      }
    }

    globalThis.URLSearchParams = URLSearchParamsPolyfill;
  }

  if (typeof URL === "undefined") {
    class URLPolyfill {
      constructor(input) {
        this.href = String(input);
      }

      toString() {
        return this.href;
      }
    }

    globalThis.URL = URLPolyfill;
  }

  class Headers {
    constructor(init) {
      this._list = [];

      if (init === undefined || init === null) {
        return;
      }

      if (init instanceof Headers) {
        this._list = cloneHeaderList(init._list);
        return;
      }

      if (
        typeof init[Symbol.iterator] === "function" &&
        typeof init !== "string"
      ) {
        for (var pair of init) {
          if (!pair || pair.length < 2) {
            throw new TypeError(
              "Each header pair must include a name and value.",
            );
          }
          this.append(pair[0], pair[1]);
        }
        return;
      }

      if (typeof init === "object") {
        var keys = Object.keys(init);
        for (var i = 0; i < keys.length; i += 1) {
          this.append(keys[i], init[keys[i]]);
        }
        return;
      }

      throw new TypeError(
        "Failed to construct Headers: unsupported initializer.",
      );
    }

    append(name, value) {
      var normalizedName = normalizeHeaderName(name);
      var normalizedValue = normalizeHeaderValue(value);
      this._list.push([normalizedName, normalizedValue]);
    }

    set(name, value) {
      var normalizedName = normalizeHeaderName(name);
      var normalizedValue = normalizeHeaderValue(value);
      this.delete(normalizedName);
      this._list.push([normalizedName, normalizedValue]);
    }

    get(name) {
      var normalizedName = normalizeHeaderName(name);
      var values = [];
      for (var i = 0; i < this._list.length; i += 1) {
        if (this._list[i][0] === normalizedName) {
          values.push(this._list[i][1]);
        }
      }
      if (values.length === 0) {
        return null;
      }
      return values.join(", ");
    }

    has(name) {
      var normalizedName = normalizeHeaderName(name);
      for (var i = 0; i < this._list.length; i += 1) {
        if (this._list[i][0] === normalizedName) {
          return true;
        }
      }
      return false;
    }

    delete(name) {
      var normalizedName = normalizeHeaderName(name);
      var next = [];
      for (var i = 0; i < this._list.length; i += 1) {
        if (this._list[i][0] !== normalizedName) {
          next.push(this._list[i]);
        }
      }
      this._list = next;
    }

    forEach(callback, thisArg) {
      for (var i = 0; i < this._list.length; i += 1) {
        callback.call(thisArg, this._list[i][1], this._list[i][0], this);
      }
    }

    *entries() {
      for (var i = 0; i < this._list.length; i += 1) {
        yield [this._list[i][0], this._list[i][1]];
      }
    }

    *keys() {
      for (var i = 0; i < this._list.length; i += 1) {
        yield this._list[i][0];
      }
    }

    *values() {
      for (var i = 0; i < this._list.length; i += 1) {
        yield this._list[i][1];
      }
    }

    [Symbol.iterator]() {
      return this.entries();
    }

    _toList() {
      return cloneHeaderList(this._list);
    }
  }

  function cloneRequest(request) {
    if (request._bodyStream !== null && request._bodyStream !== undefined) {
      throw new TypeError("Cannot clone a request with a streaming body.");
    }
    var next = Object.create(Request.prototype);
    next.method = request.method;
    next.url = request.url;
    next.headers = new Headers(request.headers);
    next.signal = request.signal;
    next._bodyBytes =
      request._bodyBytes === null ? null : copyArrayBuffer(request._bodyBytes);
    next._bodyText = request._bodyText;
    next._bodyStream = null;
    next._streamHandle = null;
    next._streamReleased = false;
    next._streamCancelled = false;
    next.body = null;
    next.bodyUsed = false;
    return next;
  }

  function initializeResponseState(response, bodyBytes, bodyText, streamHandle) {
    response._bodyBytes = bodyBytes;
    response._bodyText = bodyText;
    response._streamHandle =
      streamHandle === undefined ? null : streamHandle;
    response._streamReleased = false;
    response._streamCancelled = false;
    response._readHandle = null;
    response._abortSignal = null;
    response._abortHandler = null;
    response._abortPromise = null;
    response._abortResolve = null;
    response._abortError = null;
  }

  function payloadStreamHandle(payload) {
    if (payload === null || payload === undefined) {
      return undefined;
    }
    if (payload.bodyStreamHandle !== undefined) {
      return payload.bodyStreamHandle;
    }
    if (payload.streamHandle !== undefined) {
      return payload.streamHandle;
    }
    return payload.bodyHandle;
  }

  function releasePayloadStream(payload) {
    var handle = payloadStreamHandle(payload);
    if (handle === null || handle === undefined) {
      return;
    }
    var release = globalThis._isola_http && globalThis._isola_http._release;
    if (typeof release !== "function") {
      return;
    }
    try {
      release(handle);
    } catch (_err) {
      // The response may already have been cancelled or released.
    }
  }

  function uploadChunkBytes(value) {
    if (isArrayBuffer(value)) return copyArrayBuffer(value);
    if (isArrayBufferView(value)) return copyViewToArrayBuffer(value);
    throw new TypeError("Request body stream chunks must be ArrayBuffer or TypedArray values.");
  }

  function pumpUpload(source, uploadHandle) {
    var reader = null;
    var iterator = null;
    if (
      typeof ReadableStream !== "undefined" &&
      source instanceof ReadableStream
    ) {
      reader = source.getReader();
    } else if (isAsyncIterable(source)) {
      iterator = source[Symbol.asyncIterator]();
    } else {
      return Promise.reject(new TypeError("Invalid request body stream."));
    }

    function next() {
      return reader !== null ? reader.read() : iterator.next();
    }

    function closeSource(reason) {
      if (reader !== null) {
        return Promise.resolve(reader.cancel(reason)).catch(function () {});
      }
      if (iterator !== null && typeof iterator.return === "function") {
        return Promise.resolve(iterator.return()).catch(function () {});
      }
      return Promise.resolve();
    }

    function writeNext() {
      return Promise.resolve(next()).then(function (item) {
        if (item.done) return undefined;
        var bytes = uploadChunkBytes(item.value);
        var writeHandle = _isola_http._writeUpload(uploadHandle, bytes);
        return _isola_async
          ._wait(writeHandle, function () {
            return _isola_http._finishUploadWrite(writeHandle);
          })
          .then(writeNext);
      });
    }

    return writeNext().then(
      function () {
        if (reader !== null) reader.releaseLock();
        try {
          _isola_http._closeUpload(uploadHandle);
        } catch (_err) {
          // The request may have been aborted after the final chunk.
        }
      },
      function (error) {
        return closeSource(error).then(function () {
          if (reader !== null) reader.releaseLock();
          try {
            _isola_http._closeUpload(uploadHandle);
          } catch (_err) {
            // The HTTP side may already have released the upload.
          }
          throw error;
        });
      },
    );
  }

  class Request {
    constructor(input, init) {
      if (input === undefined) {
        throw new TypeError("Failed to construct Request: input is required.");
      }

      init = init || {};
      var source = input instanceof Request ? input : null;

      this.method = normalizeMethod(source ? source.method : "GET");
      if (init.method !== undefined) {
        this.method = normalizeMethod(init.method);
      }

      this.url = source ? source.url : normalizeUrl(input);

      var headersInit =
        init.headers !== undefined
          ? init.headers
          : source
            ? source.headers
            : undefined;
      this.headers = new Headers(headersInit);

      var signal =
        init.signal !== undefined ? init.signal : source ? source.signal : null;
      if (signal === null || signal === undefined) {
        signal = new AbortController().signal;
      }
      if (!(signal instanceof AbortSignal)) {
        throw new TypeError("Request signal must be an AbortSignal.");
      }
      this.signal = signal;

      var bodyInit = null;
      if (hasOwn.call(init, "body")) {
        bodyInit = init.body;
      } else if (source) {
        if (source.bodyUsed) {
          throw new TypeError("Cannot construct a Request with a used body.");
        }
        if (source._bodyStream !== null && source._bodyStream !== undefined) {
          throw new TypeError("Cannot construct a Request from a streaming body.");
        }
        bodyInit =
          source._bodyBytes !== null
            ? source._bodyBytes
            : source._bodyText !== null
              ? source._bodyText
              : null;
      }

      if (
        (this.method === "GET" || this.method === "HEAD") &&
        bodyInit !== null &&
        bodyInit !== undefined
      ) {
        throw new TypeError("Request with GET/HEAD method cannot have body.");
      }

      var normalizedBody = normalizeBody(bodyInit, this.headers, true);
      this._bodyBytes = normalizedBody.bytes;
      this._bodyText = normalizedBody.text;
      this._bodyStream = normalizedBody.stream;
      this.body = normalizedBody.stream;
      this.bodyUsed = false;
    }

    clone() {
      if (this.bodyUsed) {
        throw new TypeError("Cannot clone a request with a consumed body.");
      }
      return cloneRequest(this);
    }

    text() {
      return textBody(this);
    }

    json() {
      return jsonBody(this);
    }

    arrayBuffer() {
      return arrayBufferBody(this);
    }
  }

  function cloneResponse(response) {
    var next = Object.create(Response.prototype);
    next.status = response.status;
    next.statusText = response.statusText;
    next.headers = new Headers(response.headers);
    next.url = response.url;
    next.ok = response.ok;
    initializeResponseState(
      next,
      response._bodyBytes === null
        ? null
        : copyArrayBuffer(response._bodyBytes),
      response._bodyText,
      null,
    );
    if (
      (response._streamHandle !== null && response._streamHandle !== undefined) ||
      response._tee !== null && response._tee !== undefined
    ) {
      var tee = response._tee;
      if (tee === null || tee === undefined) {
        tee = createResponseTee(response);
        response._tee = tee;
        response._teeBranch = true;
        response.body = tee.addBranch(response);
      }
      next._teeBranch = true;
      next._tee = tee;
      next.body = tee.addBranch(next);
    } else {
      next.body = makeResponseBody(next);
    }
    next.bodyUsed = false;
    return next;
  }

  class Response {
    constructor(body, init) {
      init = init || {};

      this.status = init.status === undefined ? 200 : Number(init.status);
      this.statusText =
        init.statusText === undefined ? "" : String(init.statusText);
      this.headers = new Headers(init.headers);
      this.url = init.url === undefined ? "" : String(init.url);
      this.ok = this.status >= 200 && this.status <= 299;

      var normalizedBody = normalizeBody(body, this.headers, false);
      initializeResponseState(this, normalizedBody.bytes, normalizedBody.text, null);
      this.body = makeResponseBody(this);
      this.bodyUsed = false;
    }

    static _fromPayload(payload) {
      var response = Object.create(Response.prototype);
      response.status = Number(payload.status || 0);
      response.statusText =
        payload.statusText === undefined ? "" : String(payload.statusText);
      response.headers = new Headers(
        payload.headersList || payload.headers || undefined,
      );
      response.url = payload.url === undefined ? "" : String(payload.url);
      response.ok = response.status >= 200 && response.status <= 299;

      var bytes = toBodyBytes(payload.bodyBytes || payload.body);
      initializeResponseState(
        response,
        bytes,
        payload.bodyText === undefined ? null : String(payload.bodyText),
        payloadStreamHandle(payload),
      );
      response.body = makeResponseBody(response);
      response.bodyUsed = false;
      return response;
    }

    clone() {
      if (this.bodyUsed) {
        throw new TypeError("Cannot clone a response with a consumed body.");
      }
      return cloneResponse(this);
    }

    text() {
      return textBody(this);
    }

    json() {
      return jsonBody(this);
    }

    arrayBuffer() {
      return arrayBufferBody(this);
    }
  }

  function fetchImpl(input, init) {
    var request =
      input instanceof Request && init === undefined
        ? input
        : new Request(input, init);

    if (request.signal.aborted) {
      return Promise.reject(abortError(request.signal.reason));
    }

    if (request.bodyUsed) {
      return Promise.reject(
        new TypeError("Request body has already been consumed."),
      );
    }

    var body = request._bodyBytes;
    var bodyStream = request._bodyStream;
    if (body !== null || bodyStream !== null) {
      request.bodyUsed = true;
    }

    var handle;
    var uploadHandle = null;
    var upload = null;
    try {
      if (bodyStream !== null && bodyStream !== undefined) {
        uploadHandle = _isola_http._openUpload(4);
        handle = _isola_http._sendStream(
          request.method,
          request.url,
          null,
          request.headers._toList(),
          uploadHandle,
          null,
        );
        upload = pumpUpload(bodyStream, uploadHandle);
      } else {
        handle = _isola_http._send(
          request.method,
          request.url,
          null,
          request.headers._toList(),
          body,
          null,
        );
      }
    } catch (err) {
      if (uploadHandle !== null) {
        try {
          _isola_http._closeUpload(uploadHandle);
        } catch (_closeError) {}
      }
      return Promise.reject(err);
    }

    var pending = _isola_async._wait(handle, function () {
      var payload;
      var recvError;
      var hasRecvError = false;
      try {
        // Always drain the handle to keep Rust pending state in sync.
        payload = _isola_http._recv(handle);
      } catch (err) {
        recvError = err;
        hasRecvError = true;
      }

      if (request.signal.aborted) {
        releasePayloadStream(payload);
        throw abortError(request.signal.reason);
      }

      if (hasRecvError) {
        throw recvError;
      }

      var response = null;
      try {
        response = Response._fromPayload(payload);
        attachAbortSignal(response, request.signal);
        return response;
      } catch (error) {
        if (response !== null) {
          releaseStream(response);
        } else {
          releasePayloadStream(payload);
        }
        throw error;
      }
    });

    function onAbort() {
      _isola_async._cancel(handle, abortError(request.signal.reason));
      if (uploadHandle !== null) {
        try {
          _isola_http._closeUpload(uploadHandle);
        } catch (_err) {}
      }
    }

    request.signal.addEventListener("abort", onAbort);
    if (request.signal.aborted) {
      onAbort();
    }

    if (upload !== null) {
      // A failed producer must reject fetch even when the native request is
      // still waiting for another body chunk. Successful uploads leave the
      // response promise in control so fetch can resolve on response headers.
      pending = Promise.race([
        pending,
        upload.then(function () {
          return pending;
        }, function (error) {
          _isola_async._cancel(handle, error);
          throw error;
        }),
      ]);
    }

    return pending.then(
      function (response) {
        request.signal.removeEventListener("abort", onAbort);
        return response;
      },
      function (error) {
        request.signal.removeEventListener("abort", onAbort);
        throw error;
      },
    );
  }

  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetchImpl;
})();
