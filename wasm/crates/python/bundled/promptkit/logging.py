try:
    from _promptkit_logging import *  # noqa: F403
except ImportError:
    from logging import debug, info, warning, error

__all__ = ["debug", "info", "warning", "error"]
