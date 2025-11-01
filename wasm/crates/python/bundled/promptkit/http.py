try:
    from promptkit._wasi.http import *  # noqa: F403 # pyright: ignore[reportWildcardImportFromLibrary]
except ImportError:
    from promptkit._generic.http import *  # type: ignore[assignment] # noqa: F403 # pyright: ignore[reportWildcardImportFromLibrary]
