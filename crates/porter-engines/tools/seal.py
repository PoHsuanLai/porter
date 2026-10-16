# Pins a tree seal exactly as src/seal.rs computes it (see that file for the text hashed).
# Usage: python3 -I tools/seal.py <extracted tree>   prints: <seal> <number of entries>
import hashlib, os, sys

def seal(root):
    lines = []
    for dirpath, dirnames, filenames in os.walk(root):
        for name in dirnames + filenames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root)
            if os.path.islink(full):
                lines.append((rel, "L %s %s\n" % (os.readlink(full), rel)))
            elif os.path.isfile(full):
                st = os.stat(full)
                mode = "x" if st.st_mode & 0o111 else "-"
                h = hashlib.sha256()
                with open(full, "rb") as f:
                    while True:
                        b = f.read(65536)
                        if not b: break
                        h.update(b)
                lines.append((rel, "F %s %d %s %s\n" % (mode, st.st_size, h.hexdigest(), rel)))
            elif os.path.isdir(full):
                pass
            else:
                raise SystemExit("unsupported entry " + full)
    lines.sort(key=lambda t: t[0].encode())
    total = hashlib.sha256()
    for _, line in lines:
        total.update(line.encode())
    return total.hexdigest(), len(lines)

if __name__ == "__main__":
    print(*seal(sys.argv[1]))
