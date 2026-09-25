"""Your own writing -> a personal dictionary for 地球桌面输入法.

    python mkpersonal.py corpus.txt OUTDIR [train]

corpus.txt: documents separated by a line holding only \\x1e (see
mkcorpus.py). With "train", every fifth document is left out (the test set of
mkitems.py), so the benchmark measures words it has not seen.

Writes, for the user's Rime folder (%APPDATA%\\EarthDesk\\ime\\rime):
  earthdesk_personal.dict.yaml   words you use, with weights from how often
  en_personal.txt                English words in the casing you write them
"""
import collections, math, os, re, sys
import jieba
from pypinyin import lazy_pinyin, Style

jieba.setLogLevel(60)


def main():
    text = open(sys.argv[1], encoding="utf-8").read()
    out = sys.argv[2]
    docs = [d for d in text.split("\n\x1e\n") if d.strip()]
    if len(sys.argv) > 3 and sys.argv[3] == "train":
        docs = [d for i, d in enumerate(docs) if i % 5 != 0]
    zh = collections.Counter()
    en = collections.defaultdict(collections.Counter)
    for d in docs:
        for para in d.split("\n"):
            for tok in jieba.cut(para):
                if re.fullmatch(r"[一-鿿]{2,6}", tok):
                    zh[tok] += 1
                elif re.fullmatch(r"[A-Za-z]{2,}", tok):
                    en[tok.lower()][tok] += 1
    os.makedirs(out, exist_ok=True)
    rows = []
    for w, n in zh.items():
        if n < 2:
            continue
        py = lazy_pinyin(w, style=Style.NORMAL, v_to_u=False, errors="ignore")
        if len(py) != len(w):
            continue
        # Weights on rime-ice's scale (common words there: 10^5 - 10^6).
        weight = int(min(2_000_000, 150_000 * math.sqrt(n)))
        rows.append((w, " ".join(py), weight))
    rows.sort(key=lambda r: -r[2])
    with open(os.path.join(out, "earthdesk_personal.dict.yaml"), "w", encoding="utf-8") as f:
        f.write("# 地球桌面输入法：从你自己的文档里学到的词（tools/ime-bench/mkpersonal.py 生成）\n")
        f.write("---\nname: earthdesk_personal\nversion: \"1\"\nsort: by_weight\n...\n")
        for w, p, n in rows:
            f.write(f"{w}\t{p}\t{n}\n")
    with open(os.path.join(out, "en_personal.txt"), "w", encoding="utf-8") as f:
        f.write("# 地球桌面输入法：你常用的英文词和它们的大小写\n")
        for low, forms in sorted(en.items()):
            if sum(forms.values()) >= 2:
                f.write(forms.most_common(1)[0][0] + "\n")
    print(len(rows), "Chinese words,", sum(1 for f in en.values() if sum(f.values()) >= 2), "English words")


main()
