import importlib
import io
import sys
import zipfile

from promptkit.http import fetch

__all__ = ["http"]

_module_type = type(sys)


class HttpImporter:
    def __init__(self, url):
        self.url = url
        self.modules = {}
        self.archive = None
        with fetch("GET", url) as r:
            body = r.read()
            if body.startswith(b"PK"):
                try:
                    zip_ = zipfile.ZipFile(io.BytesIO(body))
                    self.archive = zipfile.Path(zip_)
                except zipfile.BadZipfile:
                    pass

    def find_spec(self, fullname, path, target=None):
        loader = self._find_module(fullname, path)
        if loader is not None:
            return importlib.util.spec_from_loader(
                fullname, loader, is_package=self.modules[fullname]["package"]
            )
        return None

    def _find_module(self, fullname, path=None):
        if fullname in self.modules:
            return self

        module_name = fullname.replace(".", "/")
        paths = [
            module_name + ".py",
            module_name + "/__init__.py",
        ]
        for path in paths:
            if self.archive is None:
                url = self.url + "/" + path
                with fetch("GET", url) as resp:
                    if resp.status >= 400:
                        continue
                    self.modules[fullname] = {
                        "content": resp.read(),
                        "filepath": url,
                        "package": path.endswith("__init__.py"),
                    }
                    return self
            else:
                try:
                    self.modules[fullname] = {
                        "content": (self.archive.joinpath(path)).read_bytes(),
                        "filepath": self.url + "#" + path,
                        "package": path.endswith("__init__.py"),
                    }
                    return self
                except FileNotFoundError:
                    continue
        return None

    def get_source(self, fullname):
        if self._find_module(fullname) is not self:
            raise ImportError(
                "Module '{}' cannot be loaded from '{}'".format(fullname, self.url)
            )
        return self.modules[fullname]["content"]

    def create_module(self, spec):
        fullname = spec.name
        if fullname not in self.modules:
            if self._find_module(fullname) is not self:
                raise ImportError(
                    "Module '{}' cannot be loaded from '{}'".format(fullname, self.url)
                )
        data = self.modules[fullname]

        mod = _module_type(fullname)
        mod.__loader__ = self
        mod.__file__ = data["filepath"]
        if data["package"]:
            mod.__path__ = ["/".join(mod.__file__.split("/")[:-1]) + "/"]
        data["module"] = mod
        return mod

    def exec_module(self, module):
        fullname = module.__name__
        sys.modules[fullname] = module
        try:
            exec(self.modules[fullname]["content"], module.__dict__)
            return module
        except:
            del sys.modules[fullname]
            raise

    def files(self):
        if self.archive is None:
            raise NotImplementedError
        return self.archive


class RepoGuard:
    def __init__(self, cls, *args, **kwargs):
        self.importer = cls(*args, **kwargs)

    def __enter__(self):
        sys.meta_path.append(self.importer)
        return self.importer

    def __exit__(self, type, value, tb):
        sys.meta_path.remove(self.importer)


def http(*args, **kwargs):
    return RepoGuard(HttpImporter, *args, **kwargs)
