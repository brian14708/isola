try:
    from promptkit._wasi.http import *  # noqa: F403
except ImportError:
    from promptkit._generic.http import *  # type: ignore # noqa: F403
