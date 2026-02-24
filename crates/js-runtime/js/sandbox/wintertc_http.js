"use strict";

(function () {
  var hasOwn = Object.prototype.hasOwnProperty;

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
      return { bytes: null, text: null };
    }

    if (isArrayBuffer(body)) {
      return { bytes: copyArrayBuffer(body), text: null };
    }

    if (isArrayBufferView(body)) {
      return { bytes: copyViewToArrayBuffer(body), text: null };
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
      return { bytes: encodeUtf8(formText), text: formText };
    }

    if (typeof body === "string") {
      if (forRequest) {
        setDefaultContentType(headers, "text/plain;charset=UTF-8");
      }
      return { bytes: encodeUtf8(body), text: body };
    }

    if (forRequest && typeof body === "object") {
      var jsonText = JSON.stringify(body);
      if (jsonText === undefined) {
        jsonText = "null";
      }
      setDefaultContentType(headers, "application/json");
      return { bytes: encodeUtf8(jsonText), text: jsonText };
    }

    var fallbackText = String(body);
    if (forRequest) {
      setDefaultContentType(headers, "text/plain;charset=UTF-8");
    }
    return { bytes: encodeUtf8(fallbackText), text: fallbackText };
  }

  function consumeBody(instance) {
    if (instance.bodyUsed) {
      return Promise.reject(new TypeError("Body has already been consumed."));
    }

    instance.bodyUsed = true;
    if (instance._bodyBytes === null) {
      return Promise.resolve(new ArrayBuffer(0));
    }
    return Promise.resolve(copyArrayBuffer(instance._bodyBytes));
  }

  function textBody(instance) {
    return consumeBody(instance).then(function (bytes) {
      if (instance._bodyText !== null && instance._bodyText !== undefined) {
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
    var next = Object.create(Request.prototype);
    next.method = request.method;
    next.url = request.url;
    next.headers = new Headers(request.headers);
    next.signal = request.signal;
    next._bodyBytes =
      request._bodyBytes === null ? null : copyArrayBuffer(request._bodyBytes);
    next._bodyText = request._bodyText;
    next.body = null;
    next.bodyUsed = false;
    return next;
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
      this.body = null;
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
    next._bodyBytes =
      response._bodyBytes === null
        ? null
        : copyArrayBuffer(response._bodyBytes);
    next._bodyText = response._bodyText;
    next.body = null;
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
      this._bodyBytes = normalizedBody.bytes;
      this._bodyText = normalizedBody.text;
      this.body = null;
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
      response._bodyBytes = bytes;
      response._bodyText =
        payload.bodyText === undefined ? null : String(payload.bodyText);
      response.body = null;
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
    if (body !== null) {
      request.bodyUsed = true;
    }

    var handle;
    try {
      handle = _isola_http._send(
        request.method,
        request.url,
        null,
        request.headers._toList(),
        body,
        null,
      );
    } catch (err) {
      return Promise.reject(err);
    }

    return _isola_async._wait(handle, function () {
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
        throw abortError(request.signal.reason);
      }

      if (hasRecvError) {
        throw recvError;
      }

      return Response._fromPayload(payload);
    });
  }

  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetchImpl;
})();
