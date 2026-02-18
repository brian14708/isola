try:
    from _isola_logging import debug, error, info, warning
except ImportError:
    import json
    import logging

    logger = logging.getLogger("sandbox")

    def _msg(message: str, *args: object, **kwargs: object) -> str:
        if args:
            message = message.format(*args)
        if kwargs:
            message += " " + json.dumps(kwargs, ensure_ascii=False)
        return message

    def debug(message: str, *args: object, **kwargs: object) -> None:
        logger.log(logging.DEBUG, _msg(message, *args, **kwargs))

    def info(message: str, *args: object, **kwargs: object) -> None:
        logger.log(logging.INFO, _msg(message, *args, **kwargs))

    def error(message: str, *args: object, **kwargs: object) -> None:
        logger.log(logging.ERROR, _msg(message, *args, **kwargs))

    def warning(message: str, *args: object, **kwargs: object) -> None:
        logger.log(logging.WARNING, _msg(message, *args, **kwargs))


__all__ = ["debug", "error", "info", "warning"]
