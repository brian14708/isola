# noqa: INP001

import pathlib
import sys
import tempfile
import zipfile

if __name__ == "__main__":
    pyzip_path = sys.argv[1] + ".zip"
    srczip_path = sys.argv[1] + "-src.zip"
    source_paths = sys.argv[2:]

    with zipfile.PyZipFile(pyzip_path, "w") as z:
        for path_str in source_paths:
            path = pathlib.Path(path_str)

            # Handle wheel files
            if path.is_file() and path.suffix == ".whl":
                # Extract wheel to temp dir and use writepy
                with tempfile.TemporaryDirectory() as tmpdir:
                    with zipfile.ZipFile(path, "r") as whl:
                        whl.extractall(tmpdir)
                    tmppath = pathlib.Path(tmpdir)
                    for item in tmppath.iterdir():
                        if item.name.endswith(".dist-info") or item.name.endswith(
                            ".data"
                        ):
                            continue
                        if (item.is_file() and item.suffix == ".py") or item.is_dir():
                            z.writepy(item)
            # Handle directories
            elif path.is_dir():
                for item in path.iterdir():
                    if item.is_file() and item.suffix != ".py":
                        continue
                    z.writepy(item)

    EXT = {".py", ".pyi", ".typed"}
    with zipfile.ZipFile(srczip_path, "w", compression=zipfile.ZIP_DEFLATED) as src_zip:
        for path_str in source_paths:
            path = pathlib.Path(path_str)

            # Handle wheel files
            if path.is_file() and path.suffix == ".whl":
                with zipfile.ZipFile(path, "r") as whl:
                    for name in whl.namelist():
                        if any(name.endswith(ext) for ext in EXT):
                            src_zip.writestr(name, whl.read(name))
            # Handle directories
            elif path.is_dir():
                for item in path.iterdir():
                    if item.name.startswith("."):
                        continue
                    if item.is_file():
                        if item.suffix in EXT:
                            arcname = str(item.relative_to(path))
                            src_zip.write(item, arcname=arcname)
                    else:
                        for file_path in item.rglob("*"):
                            if file_path.is_file() and file_path.suffix in EXT:
                                arcname = str(file_path.relative_to(path))
                                src_zip.write(file_path, arcname=arcname)
