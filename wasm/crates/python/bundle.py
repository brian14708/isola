import zipfile
import sys
import os

if __name__ == "__main__":
    with zipfile.PyZipFile(sys.argv[1], "w") as z:
        for dir in sys.argv[2:]:
            for f in os.listdir(dir):
                path = os.path.join(dir, f)
                if os.path.isfile(path) and os.path.splitext(path)[1] != ".py":
                    continue
                z.writepy(os.path.join(dir, f))
