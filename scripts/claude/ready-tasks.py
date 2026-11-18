#!/usr/bin/env python3
"""Tasks ready on origin/develop: todo, no needs:user, dependencies done, no open PR.
Usage: ready.py [in-flight-ids...]  -> prints JSON [{id,title,phase}] sorted by phase and id."""
import json, re, subprocess, sys
repo = __import__("os").path.dirname(__import__("os").path.dirname(__import__("os").path.dirname(__import__("os").path.abspath(__file__))))
def sh(*a): return subprocess.run(a, cwd=repo, capture_output=True, text=True).stdout
sh("git", "fetch", "-q", "origin")
files = [f for f in sh("git", "ls-tree", "--name-only", "origin/develop", "docs/tasks/").split() if re.search(r"T-\d+\.md$", f)]
tasks = {}
for f in files:
    txt = sh("git", "show", f"origin/develop:{f}")
    fm = txt.split("---")[1]
    get = lambda k: (re.search(rf"^{k}:\s*(.*)$", fm, re.M) or [None, ""])[1].strip()
    tasks[get("id")] = dict(id=get("id"), title=get("title").strip('"'), phase=get("phase"), status=get("status"),
                            needs=get("needs"), deps=re.findall(r"T-\d+", get("depends_on")))
open_prs = {p["headRefName"].split("/")[-1] for p in json.loads(sh("gh", "pr", "list", "--state", "open", "--json", "headRefName"))}
busy = set(sys.argv[1:]) | open_prs
ready = [t for t in tasks.values() if t["status"] == "todo" and "user" not in t["needs"] and t["id"] not in busy
         and t["id"] != "T-062" and all(tasks.get(d, {}).get("status") == "done" for d in t["deps"])]
ready.sort(key=lambda t: (t["phase"], t["id"]))
print(json.dumps([{k: t[k] for k in ("id", "title", "phase", "needs")} for t in ready], ensure_ascii=False))
