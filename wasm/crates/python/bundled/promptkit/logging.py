try:
    from _promptkit_logging import debug, error, info, warning
except ImportError:
    import json
    import logging

    def _msg(message: str, *args: object, **kwargs: object) -> str:
        if args:
            message = message.format(*args)
        if kwargs:
            message += " " + json.dumps(kwargs, ensure_ascii=False)
        return message

    def debug(message: str, *args: object, **kwargs: object) -> None:
        logging.debug(_msg(message, *args, **kwargs))

    def info(message: str, *args: object, **kwargs: object) -> None:
        logging.info(_msg(message, *args, **kwargs))

    def error(message: str, *args: object, **kwargs: object) -> None:
        logging.error(_msg(message, *args, **kwargs))

    def warning(message: str, *args: object, **kwargs: object) -> None:
        logging.warning(_msg(message, *args, **kwargs))


__all__ = ["debug", "info", "warning", "error"]
