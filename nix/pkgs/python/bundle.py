# noqa: INP001

import pathlib
import sys
import tempfile
import zipfile

if __name__ == "__main__":
    pyzip_path = sys.argv[1] + ".zip"
    source_paths = sys.argv[2:]

    with zipfile.PyZipFile(pyzip_path, "w") as z:
        z._strict_timestamps = False  # noqa: SLF001
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
