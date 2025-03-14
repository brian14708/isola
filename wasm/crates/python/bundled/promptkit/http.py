try:
    from promptkit._wasi.http import *
except ImportError:
    from promptkit._generic.http import *
