try:
    from _promptkit_logging import debug, error, info, warning
except ImportError:
    import json
    import logging

    def _msg(message: str, *args, **kwargs) -> str:
        if args:
            message = message.format(*args)
        if kwargs:
            message += " " + json.dumps(kwargs, ensure_ascii=False)
        return message

    def debug(message: str, *args, **kwargs) -> None:
        logging.debug(_msg(message, *args, **kwargs))

    def info(message: str, *args, **kwargs) -> None:
        logging.info(_msg(message, *args, **kwargs))

    def error(message: str, *args, **kwargs) -> None:
        logging.error(_msg(message, *args, **kwargs))

    def warning(message: str, *args, **kwargs) -> None:
        logging.warning(_msg(message, *args, **kwargs))


__all__ = ["debug", "info", "warning", "error"]
