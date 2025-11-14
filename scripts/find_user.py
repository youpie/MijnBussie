#!/usr/bin/env python3
import os
import sys
import tempfile
from pathlib import Path
from dotenv import dotenv_values

BASE_DIR = Path.home() / "Services" / "Webcom"
TMP_FILE = Path(tempfile.gettempdir()) / "kuma-find.tmp"
################
#
# !! GROOTENDEELS AI
#
################
def parse_env(path):
    vars = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k,v = line.split("=",1)
        vars[k.strip()] = v.strip().strip('"').strip("'")
    return vars


def collect_env_maps(base):
    email_map = {}
    user_map = {}
    for folder in base.iterdir():
        if not folder.is_dir(): continue
        env_file = folder / ".env"
        if not env_file.exists(): continue
        env = parse_env(env_file)
        if "MAIL_TO" in env:
            for e in env["MAIL_TO"].split(","):
                email_map.setdefault(e.strip().lower(), []).append(str(folder))
        if "USERNAME" in env:
            user_map.setdefault(env["USERNAME"].lower(), []).append(str(folder))
    return email_map, user_map

def collect_name_map(base):
    nm = {}
    for dirpath, dirnames, filenames in os.walk(base):
        rel = Path(dirpath).relative_to(base)
        if len(rel.parts) > 3:
            dirnames[:] = []
            continue
        kuma = Path(dirpath) / "kuma" / "name"
        if kuma.exists():
            nm.setdefault(kuma.read_text().strip().lower(), []).append(dirpath)
    return nm

def levenshtein(s1, s2):
    len1, len2 = len(s1), len(s2)
    d = list(range(len2+1))
    for i in range(1, len1+1):
        prev, d[0] = d[0], i
        for j in range(1, len2+1):
            cur = d[j]
            cost = 0 if s1[i-1]==s2[j-1] else 1
            d[j] = min(prev+cost, d[j]+1, d[j-1]+1)
            prev = cur
    return d[len2]

def choose_from_list(q, options):
    print(f"Multiple matches for “{q}”:")
    for i,d in enumerate(options,1):
        print(f"  {i}) {d}")
    choice = input(f"Choose [1-{len(options)}]: ")
    try:
        idx = int(choice)-1
        if 0 <= idx < len(options):
            return options[idx]
    except:
        pass
    print("Invalid choice – aborting.")
    return None

def write_tmp(path_str):
    if path_str:
        TMP_FILE.write_text(path_str)
    elif TMP_FILE.exists():
        TMP_FILE.unlink()

def main():
    if len(sys.argv) < 2:
        print("Usage: kuma_find.py \"search\"")
        sys.exit(1)
    q = sys.argv[1]
    lc = q.lower()

    email_map, user_map = collect_env_maps(BASE_DIR)
    name_map = collect_name_map(BASE_DIR)
    email = False
    matches = []
    # 1) email
    if "@" in q:
        matches = email_map.get(lc, [])
        email = True
    # 2) username
    elif q and q[0].isdigit():
        matches = user_map.get(lc, [])
    # 3) regular name
    else:
        matches = name_map.get(lc, [])

    folder = None
    n = len(matches)
    if n == 1:
        folder = matches[0]
        print(f"Found: {folder}")
    elif n == 2:
        if matches[0][0] == "_":
            folder = matches[1]
        elif matches[1][0] == "_":
            folder = matches[0]
        else:
            folder = choose_from_list(q, matches)
            if folder:
                print(f"You picked: {folder}")
    elif n > 2:
        folder = choose_from_list(q, matches)
        if folder:
            print(f"You picked: {folder}")

    # 4) fallback via Levenshtein on names only if no folder yet
    if not folder and "@" not in q and not (q and q[0].isdigit()):
        best = None
        best_dist = 1e9
        for name, dirs in name_map.items():
            dist = levenshtein(name, lc)
            if dist < best_dist:
                best_dist = dist
                best = dirs[0]
        if best:
            print(f"No exact match for “{q}”. Closest: {best}")
            folder = None  # do not auto-cd on fallback

    write_tmp(folder if folder else "")
    if not email:
        env = dotenv_values(f"{folder}/.env")
        email = env.get("MAIL_TO", "")
        print(email)
    filename = env.get("RANDOM_FILENAME", "")
    print("{0}.ics".format(filename))
    sys.exit(0 if folder else 1)

if __name__=="__main__":
    main()
