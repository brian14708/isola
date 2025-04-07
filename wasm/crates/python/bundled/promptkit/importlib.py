import importlib
import importlib.abc
import importlib.util
import io
import sys
import zipfile
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, override

from promptkit.http import fetch

if TYPE_CHECKING:
    import types
    from collections.abc import Callable, Sequence
    from importlib.machinery import ModuleSpec

__all__ = ["http"]

_module_type = type(sys)


@dataclass
class ModuleInfo:
    content: str
    filepath: str
    package: bool
    module: "types.ModuleType | None" = None


class HttpImporter(
    importlib.abc.MetaPathFinder,
    importlib.abc.InspectLoader,
    importlib.abc.TraversableResources,
):
    def __init__(self, url: str) -> None:
        self.url: str = url
        self.modules: dict[str, ModuleInfo] = {}
        self.archive: zipfile.Path | None = None
        with fetch("GET", url) as r:
            body = r.read()
            if body.startswith(b"PK"):
                try:
                    zip_ = zipfile.ZipFile(io.BytesIO(body))
                    self.archive = zipfile.Path(zip_)
                except zipfile.BadZipfile:
                    pass

    @override
    def find_spec(
        self,
        fullname: str,
        path: "Sequence[str] | None" = None,
        target: "types.ModuleType | None" = None,
    ) -> "ModuleSpec | None":
        loader = self._find_module(fullname, path)
        if loader is not None:
            return importlib.util.spec_from_loader(
                fullname, loader, is_package=self.modules[fullname].package
            )
        return None

    def _find_module(
        self, fullname: str, path: "Sequence[str] | None" = None
    ) -> "HttpImporter | None":
        if fullname in self.modules:
            return self

        module_name: str = fullname.replace(".", "/")
        paths: list[str] = [
            module_name + ".py",
            module_name + "/__init__.py",
        ]
        for path_entry in paths:
            if self.archive is None:
                url: str = self.url + "/" + path_entry
                with fetch("GET", url) as resp:
                    if resp.status >= 400:
                        continue
                    self.modules[fullname] = ModuleInfo(
                        content=resp.read().decode(),
                        filepath=url,
                        package=path_entry.endswith("__init__.py"),
                    )
                    return self
            else:
                try:
                    self.modules[fullname] = ModuleInfo(
                        content=(self.archive.joinpath(path_entry))
                        .read_bytes()
                        .decode(),
                        filepath=self.url + "#" + path_entry,
                        package=path_entry.endswith("__init__.py"),
                    )
                    return self
                except FileNotFoundError:
                    continue
        return None

    @override
    def get_source(self, fullname: str) -> str:
        if self._find_module(fullname) is not self:
            raise ImportError(f"Module '{fullname}' cannot be loaded from '{self.url}'")
        return self.modules[fullname].content

    @override
    def create_module(self, spec: "ModuleSpec") -> "types.ModuleType":
        fullname: str = spec.name
        if fullname not in self.modules and self._find_module(fullname) is not self:
            raise ImportError(f"Module '{fullname}' cannot be loaded from '{self.url}'")
        data: ModuleInfo = self.modules[fullname]

        mod: types.ModuleType = _module_type(fullname)
        mod.__loader__ = self
        mod.__file__ = data.filepath
        if data.package:
            mod.__path__ = ["/".join(mod.__file__.split("/")[:-1]) + "/"]
        data.module = mod
        return mod

    @override
    def exec_module(self, module: "types.ModuleType") -> None:
        fullname: str = module.__name__
        sys.modules[fullname] = module
        try:
            exec(self.modules[fullname].content, module.__dict__)
        except Exception:
            del sys.modules[fullname]
            raise

    @override
    def files(self) -> zipfile.Path:
        if self.archive is None:
            raise NotImplementedError
        return self.archive


class RepoGuard[T: importlib.abc.MetaPathFinder, **P]:
    def __init__(self, cls: "Callable[P, T]", *args: P.args, **kwargs: P.kwargs):
        self.importer: T = cls(*args, **kwargs)

    def __enter__(self) -> T:
        sys.meta_path.append(self.importer)
        return self.importer

    def __exit__(self, *_: object) -> bool | None:
        sys.meta_path.remove(self.importer)
        return None


def http(*args: Any, **kwargs: Any) -> RepoGuard[HttpImporter, ...]:
    return RepoGuard(HttpImporter, *args, **kwargs)
