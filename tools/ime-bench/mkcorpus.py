"""Collect your own writing from Inbox (data/inbox.sqlite3: memos + wiki
pages) into one text file for mkitems.py / mkpersonal.py. Read-only.

    python mkcorpus.py <Inbox folder> corpus.txt
"""
import os, re, sqlite3, sys


def clean(t):
    t = re.sub(r"```.*?```", "", t, flags=re.S)
    t = re.sub(r"`[^`]*`", "", t)
    t = re.sub(r"!?\[\[([^\]|]*\|)?([^\]]*)\]\]", r"\2", t)
    t = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", t)
    t = re.sub(r"https?://\S+", "", t)
    t = re.sub(r"<[^>]+>", "", t)
    t = re.sub(r"^[#>\-\*\+\s|]+", "", t, flags=re.M)
    return t


def main():
    db = os.path.join(sys.argv[1], "data", "inbox.sqlite3")
    c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    cols = [r[1] for r in c.execute("pragma table_info(memos)")]
    q = "select body from memos" + (" where deleted_at is null" if "deleted_at" in cols else "")
    docs = [r[0] for r in c.execute(q)] + [r[0] for r in c.execute("select body from wiki_pages")]
    out = [clean(d) for d in docs if d and d.strip()]
    with open(sys.argv[2], "w", encoding="utf-8") as f:
        f.write("\n\x1e\n".join(out))
    print(len(out), "documents")


main()
