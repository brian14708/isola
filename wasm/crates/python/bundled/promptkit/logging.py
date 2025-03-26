try:
    from _promptkit_logging import *  # noqa: F403
except ImportError:
    from logging import debug, error, info, warning

__all__ = ["debug", "info", "warning", "error"]
