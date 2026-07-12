import re, sys
lines = open('disclosure.md').read().split('\n')
def is_drop_single(l):
    s = l.strip()
    return (s.startswith('- **Filing note:**') or s.startswith('- **AAPA notation:**') or
        s.startswith('> **Filing note — MANDATORY:**') or s.startswith('> **Document-handling note') or
        s.startswith('*Prosecution-workflow notes in this section') or
        s.startswith('*A formal prior art search is recommended before filing. In particular') or
        l.startswith('**Conception Date:**') or l.startswith('**Disclosure Date:**') or
        l.startswith('**Prior Public Disclosure:**') or l.startswith('**Prior Sales:**'))
def h(s): return lambda l: l.strip()==s
blocks = [
    (h('### §103 Combination Analysis'), h('### Comparison Matrix')),
    (h("### Scope of Inventor's Awareness — Filing Note"), h('### 11. HRV-Aggregating Consumer Wellness Platforms (Inventor Awareness)')),
    (h('### 35 USC 101 Analysis — Pre-Draft'), lambda l: l.startswith('**Claim term definition')),
    (h('## Claim-to-Code Mapping'), lambda l: False),
]
out=[]; i=0; n=len(lines)
while i<n:
    l=lines[i]; matched=False
    for sp,tp in blocks:
        if sp(l):
            j=i+1
            while j<n and not tp(lines[j]): j+=1
            i=j; matched=True; break
    if matched: continue
    if is_drop_single(l): i+=1; continue
    out.append(l); i+=1
t='\n'.join(out)
t=re.sub(r'\n{3,}','\n\n',t).lstrip('\n').rstrip('\n')
if t.endswith('---'): t=t[:-3].rstrip('\n')
open('provisional_filing_copy.md','w').write(t+'\n')
print("regenerated provisional_filing_copy.md:", len(t.splitlines())+1, "lines")
