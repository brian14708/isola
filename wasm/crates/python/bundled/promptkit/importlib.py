import importlib
import io
import sys
import zipfile
import re

from promptkit.http import fetch

__all__ = ["http", "pypi"]

_module_type = type(sys)


def _fetch_pypi(module_name, version, pypi_url):
    url = pypi_url + module_name + "/"
    with fetch("GET", url) as r:
        urls = re.findall(r'(?<= href=")[^"]+\.whl', r.text())
        for f in reversed(urls):
            if version is None or version in f:
                return f if f.startswith("http") else url + f
        return None


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
                with fetch("GET", url) as r:
                    if r.status >= 400:
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

    def files():
        if self.archive is None:
            raise NotImplementedError
        return self.archive


class PyPIImporter:
    def __init__(self, index_url=None, replace={}):
        self.url = index_url or "https://pypi.org/simple/"
        self.module_importers = {}
        self.replace = replace

    def _find_module(self, module_name, path=None):
        module_root = module_name.split(".")[0]
        if module_root in self.module_importers:
            return self.module_importers[module_root]

        project_name, version = self.replace.get(module_root, (module_root, None))
        if project_name is None or project_name.startswith("_"):
            return None

        url = _fetch_pypi(project_name, version, self.url)
        if url is None:
            return None

        importer = HttpImporter(url)
        found = importer._find_module(module_name)
        self.module_importers[module_root] = found
        return found

    def find_spec(self, fullname, path, target=None):
        loader = self._find_module(fullname, path)
        if loader is not None:
            return loader.find_spec(fullname, path, target)
        return None


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


def pypi(*args, **kwargs):
    return RepoGuard(PyPIImporter, *args, **kwargs)
