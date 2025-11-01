# noqa: INP001

import pathlib
import sys
import zipfile

if __name__ == "__main__":
    pyzip_path = sys.argv[1] + ".zip"
    srczip_path = sys.argv[1] + "-src.zip"
    source_dirs = sys.argv[2:]

    with zipfile.PyZipFile(pyzip_path, "w") as z:
        for dir_ in source_dirs:
            dir_path = pathlib.Path(dir_)
            for item in dir_path.iterdir():
                if item.is_file() and item.suffix != ".py":
                    continue
                z.writepy(item)

    EXT = {".py", ".pyi", ".typed"}
    with zipfile.ZipFile(srczip_path, "w", compression=zipfile.ZIP_DEFLATED) as src_zip:
        for dir_ in source_dirs:
            dir_path = pathlib.Path(dir_)
            for item in dir_path.iterdir():
                if item.name.startswith("."):
                    continue
                if item.is_file():
                    if item.suffix in EXT:
                        arcname = str(item.relative_to(dir_path))
                        src_zip.write(item, arcname=arcname)
                else:
                    for file_path in item.rglob("*"):
                        if file_path.is_file() and file_path.suffix in EXT:
                            arcname = str(file_path.relative_to(dir_path))
                            src_zip.write(file_path, arcname=arcname)
