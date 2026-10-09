from pathlib import Path
import sys

p = Path(sys.argv[1])
p.write_text("".join(f"root;bucket{i % 100};work{i} {1000 if i == 4242 else 1}\n" for i in range(20000)))

lines = p.read_text().splitlines()
assert len(lines) == 20000
assert sum(int(line.rsplit(" ", 1)[1]) for line in lines) == 20999
assert lines[4242] == "root;bucket42;work4242 1000"
