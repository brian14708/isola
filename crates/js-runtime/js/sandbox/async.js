// Async infrastructure: bridges WASI pollables to JS Promises.
//
// The Rust event loop in script.rs drives the poll cycle:
//   1. Execute microtasks (pending QuickJS jobs)
//   2. If promise unresolved and pending ops exist -> Rust calls wasi:io/poll::poll
//   3. Rust calls _isola_async._resolve(readyHandles)
//   4. JS resolves promises, creating new microtasks -> repeat

"use strict";

globalThis._isola_async = {
  _pending: new Map(), // handle -> {resolve, reject, getResult}
  _raw_sleep: null, // native _isola_sys.sleep(handle) before Promise wrapper

  // Register a pending operation. Returns a Promise that resolves
  // when the Rust event loop detects the pollable is ready.
  // getResult: function() that retrieves the actual result (called on resolution)
  _wait: function (handle, getResult) {
    return new Promise(function (resolve, reject) {
      _isola_async._pending.set(handle, {
        resolve: resolve,
        reject: reject,
        getResult: getResult,
      });
    });
  },

  // Called by the Rust event loop after wasi:io/poll::poll returns.
  // readyHandles: Array<u32> of handles whose pollables are ready.
  _resolve: function (readyHandles) {
    for (var i = 0; i < readyHandles.length; i++) {
      var h = readyHandles[i];
      var entry = _isola_async._pending.get(h);
      if (entry) {
        _isola_async._pending.delete(h);
        try {
          entry.resolve(entry.getResult());
        } catch (e) {
          entry.reject(e);
        }
      }
    }
  },

  has_pending: function () {
    return _isola_async._pending.size > 0;
  },
};

// Wrap _isola_sys.hostcall to return a Promise.
// The Rust side now returns a pollable handle (u32) instead of blocking.
(function () {
  var _raw_hostcall = _isola_sys.hostcall;
  _isola_sys.hostcall = function (callType, payload) {
    var handle = _raw_hostcall(callType, payload);
    return _isola_async._wait(handle, function () {
      return _isola_sys._finish_hostcall(handle);
    });
  };
})();

// Wrap _isola_sys.sleep to return a Promise.
// The Rust side returns a pollable handle (u32) instead of blocking.
(function () {
  var _raw_sleep = _isola_sys.sleep;
  _isola_async._raw_sleep = _raw_sleep;
  _isola_sys.sleep = function (duration) {
    var handle = _raw_sleep(duration);
    return _isola_async._wait(handle, function () {
      _isola_sys._finish_sleep(handle);
    });
  };
})();

// Minimal timer polyfill backed by pollable sleeps.
(function () {
  var _timers = new Map(); // timerId -> {handle, settled, repeat, delaySeconds, callback, args}
  var _nextTimerId = 1;

  function nextTimerId() {
    while (_timers.has(_nextTimerId)) {
      _nextTimerId += 1;
      if (_nextTimerId > 2147483647) {
        _nextTimerId = 1;
      }
    }
    var timerId = _nextTimerId;
    _nextTimerId += 1;
    if (_nextTimerId > 2147483647) {
      _nextTimerId = 1;
    }
    return timerId;
  }

  function normalizeDelaySeconds(delay) {
    var millis = Number(delay);
    if (!Number.isFinite(millis) || millis < 0) {
      millis = 0;
    }
    return millis / 1000;
  }

  function scheduleTimer(timerId, timer) {
    var handle = _isola_async._raw_sleep(timer.delaySeconds);
    timer.handle = handle;

    _isola_async
      ._wait(handle, function () {
        _isola_sys._finish_sleep(handle);
      })
      .then(
        function () {
          if (timer.settled) {
            return;
          }
          if (timer.repeat && _timers.has(timerId)) {
            scheduleTimer(timerId, timer);
          } else {
            timer.settled = true;
            _timers.delete(timerId);
          }
          timer.callback.apply(globalThis, timer.args);
        },
        function (err) {
          timer.settled = true;
          _timers.delete(timerId);
          throw err;
        },
      );
  }

  function setTimer(callback, delay, repeat, args) {
    if (typeof callback !== "function") {
      throw new TypeError("timer callback must be a function");
    }

    var timerId = nextTimerId();
    var timer = {
      handle: 0,
      settled: false,
      repeat: repeat,
      delaySeconds: normalizeDelaySeconds(delay),
      callback: callback,
      args: args,
    };
    _timers.set(timerId, timer);
    scheduleTimer(timerId, timer);
    return timerId;
  }

  function clearTimer(timerId) {
    var numericId = Number(timerId);
    if (!Number.isFinite(numericId)) {
      return;
    }

    var timer = _timers.get(numericId);
    if (!timer || timer.settled) {
      return;
    }

    timer.settled = true;
    _timers.delete(numericId);

    // Remove this wait entry from both JS and Rust registries.
    _isola_async._pending.delete(timer.handle);
    try {
      _isola_sys._finish_sleep(timer.handle);
    } catch (_err) {
      // A near-simultaneous resolve may have already consumed the handle.
    }
  }

  globalThis.setTimeout = function (callback, delay) {
    return setTimer(
      callback,
      delay,
      false,
      Array.prototype.slice.call(arguments, 2),
    );
  };

  globalThis.clearTimeout = clearTimer;

  globalThis.setInterval = function (callback, delay) {
    return setTimer(
      callback,
      delay,
      true,
      Array.prototype.slice.call(arguments, 2),
    );
  };

  globalThis.clearInterval = clearTimer;
})();
