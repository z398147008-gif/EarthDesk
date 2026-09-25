"""Corpus -> benchmark items: one line per token.
  Z <code> <word>   Chinese word, 微软双拼 code
  E <code> <word>   English word (code = lower-case letters)
  C - <text>        other text (punctuation, numbers...): committed as is, not scored
  #                 document boundary
"""
import re, sys, random, jieba
from pypinyin import lazy_pinyin, Style
jieba.setLogLevel(60)

RULES = [
    (r"iu$", "Q"), (r"[iu]a$", "W"), (r"er$|[uv]an$", "R"), (r"[uv]e$", "T"), (r"v$|uai$", "Y"),
    (r"^sh", "U"), (r"^ch", "I"), (r"^zh", "V"), (r"uo$", "O"), (r"[uv]n$", "P"),
    (r"(.)i?ong$", r"\1S"), (r"[iu]ang$", "D"), (r"(.)en$", r"\1F"), (r"(.)eng$", r"\1G"),
    (r"(.)ang$", r"\1H"), (r"ian$", "M"), (r"(.)an$", r"\1J"), (r"iao$", "C"), (r"(.)ao$", r"\1K"),
    (r"(.)ai$", r"\1L"), (r"(.)ei$", r"\1Z"), (r"ie$", "X"), (r"ui$", "V"), (r"(.)ou$", r"\1B"),
    (r"in$", "N"), (r"ing$", ";"),
]

def mspy(syl):
    s = syl.lower().replace("ü", "v")
    if s and s[0] in "aoe":
        s = "o" + s
    for pat, rep in RULES:
        s = re.sub(pat, rep, s)
    return s.lower()

def chunks(words):
    """Group jieba words into the phrases one types in one go: at least 2
    and at most 6 characters, never across punctuation."""
    cur = ""
    for w in words:
        if len(cur) + len(w) > 6 and cur:
            yield cur
            cur = ""
        cur += w
        if len(cur) >= 2:
            yield cur
            cur = ""
    if cur:
        yield cur

def items(doc):
    for para in doc.split("\n"):
        run = []
        def flush():
            for ch in chunks(run):
                py = lazy_pinyin(ch, style=Style.NORMAL, v_to_u=False, errors="ignore")
                if len(py) == len(ch):
                    yield ("Z", "".join(mspy(p) for p in py), ch)
                else:
                    yield ("C", "-", ch)
            run.clear()
        for tok in jieba.cut(para):
            tok = tok.strip()
            if not tok:
                continue
            if re.fullmatch(r"[\u4e00-\u9fff]+", tok):
                run.append(tok)
                continue
            yield from flush()
            if re.fullmatch(r"[A-Za-z]{2,}", tok):
                yield ("E", tok.lower(), tok)
            else:
                yield ("C", "-", tok)
        yield from flush()

def main():
    text = open(sys.argv[1], encoding="utf-8").read()
    docs = [d for d in text.split("\n\x1e\n") if d.strip()]
    part = sys.argv[2]          # train | test
    limit = int(sys.argv[3])
    out = open(sys.argv[4], "w", encoding="utf-8")
    # Every fifth document is held out for testing.
    sel = [d for i, d in enumerate(docs) if (i % 5 == 0) == (part == "test")]
    random.seed(7)
    random.shuffle(sel)
    n = 0
    for d in sel:
        out.write("#\n")
        for k, c, t in items(d):
            out.write(f"{k}\t{c}\t{t}\n")
            if k != "C":
                n += 1
        if n >= limit:
            break
    print(part, n, "scored items")

main()
