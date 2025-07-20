import os
import sys
import zipfile

if __name__ == "__main__":
    pyzip_path = sys.argv[1] + ".zip"
    srczip_path = sys.argv[1] + "-src.zip"
    source_dirs = sys.argv[2:]

    with zipfile.PyZipFile(pyzip_path, "w") as z:
        for dir in source_dirs:
            for f in os.listdir(dir):
                path = os.path.join(dir, f)
                if os.path.isfile(path) and os.path.splitext(path)[1] != ".py":
                    continue
                z.writepy(path)

    EXT = {".py", ".pyi", ".typed"}
    with zipfile.ZipFile(srczip_path, "w", compression=zipfile.ZIP_DEFLATED) as src_zip:
        for dir in source_dirs:
            for subdir in os.listdir(dir):
                if subdir.startswith("."):
                    continue
                path = os.path.join(dir, subdir)
                if os.path.isfile(path):
                    ext = os.path.splitext(subdir)[1]
                    if ext in EXT:
                        arcname = os.path.relpath(path, start=dir)
                        src_zip.write(path, arcname=arcname)
                else:
                    for root, _, files in os.walk(path):
                        for f in files:
                            ext = os.path.splitext(f)[1]
                            if ext in EXT:
                                full_path = os.path.join(root, f)
                                arcname = os.path.relpath(full_path, start=dir)
                                src_zip.write(full_path, arcname=arcname)
