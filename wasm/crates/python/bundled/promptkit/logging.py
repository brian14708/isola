try:
    from _promptkit_logging import *
except ImportError:
    from logging import debug, info, warning, error

__all__ = ["debug", "info", "warning", "error"]
