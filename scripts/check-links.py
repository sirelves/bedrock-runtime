import re, pathlib, sys
def slug(h):
    s = h.strip().lower().replace('`','')
    s = re.sub(r'[^\w\s-]', '', s, flags=re.UNICODE)
    return s.replace(' ', '-')
files = [f for f in pathlib.Path('.').rglob('*.md') if '.git' not in f.parts and f.name != 'plan.md']
heads = {f.resolve(): {slug(m) for m in re.findall(r'^#{1,6}\s+(.*)$', f.read_text(), re.M)} for f in files}
bad = []
for f in files:
    t = f.read_text()
    for target, anchor in re.findall(r'\]\(([^)#\s]*)#([^)\s]+)\)', t):
        dest = (f.parent / target).resolve() if target else f.resolve()
        if dest not in heads: bad.append(f"{f}: arquivo inexistente -> {target}")
        elif anchor not in heads[dest]: bad.append(f"{f} -> {target}#{anchor}")
    for target in re.findall(r'\]\(([^)#\s]+\.md)\)', t):
        if not (f.parent / target).exists(): bad.append(f"{f}: link quebrado -> {target}")
print("\n".join(bad) if bad else "todos os links internos ok")
sys.exit(1 if bad else 0)
