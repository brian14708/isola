"use strict";

(function () {
  function createAbortError(reason) {
    var message =
      reason === undefined ? "The operation was aborted." : String(reason);
    var error = new Error(message);
    error.name = "AbortError";
    return error;
  }

  function createAbortEvent(signal) {
    return {
      type: "abort",
      target: signal,
      currentTarget: signal,
    };
  }

  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = [];
    }

    addEventListener(type, listener) {
      if (type !== "abort" || typeof listener !== "function") {
        return;
      }
      this._listeners.push(listener);
    }

    removeEventListener(type, listener) {
      if (type !== "abort") {
        return;
      }
      for (var i = 0; i < this._listeners.length; i += 1) {
        if (this._listeners[i] === listener) {
          this._listeners.splice(i, 1);
          return;
        }
      }
    }

    throwIfAborted() {
      if (this.aborted) {
        throw createAbortError(this.reason);
      }
    }

    _triggerAbort(reason) {
      if (this.aborted) {
        return;
      }

      this.aborted = true;
      this.reason = reason;

      var event = createAbortEvent(this);
      var listeners = this._listeners.slice();
      for (var i = 0; i < listeners.length; i += 1) {
        try {
          listeners[i].call(this, event);
        } catch (_err) {
          // Best effort event dispatch; listeners are isolated.
        }
      }

      if (typeof this.onabort === "function") {
        try {
          this.onabort.call(this, event);
        } catch (_err) {
          // Same isolation behavior as listener callbacks.
        }
      }
    }

    static abort(reason) {
      var controller = new AbortController();
      controller.abort(reason);
      return controller.signal;
    }
  }

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }

    abort(reason) {
      this.signal._triggerAbort(reason);
    }
  }

  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = AbortController;
  globalThis.__isolaAbortError = createAbortError;
})();
