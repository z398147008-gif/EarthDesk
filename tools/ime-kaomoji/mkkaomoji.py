"""Build the input method's kaomoji (颜文字) table, ime/engine/data/kaomoji.tsv.

    python mkkaomoji.py [MOZC_DIR]

Sources, most popular first:
  popular.tsv      the ones common in Japan in recent years, with Chinese and
                   Japanese keywords (edit this file to add more);
  Mozc's emoticon data (Google Japanese Input's list, BSD-3, see
  LICENSE-mozc.txt): src/data/emoticon/categorized.tsv and emoticon.tsv,
  read from MOZC_DIR or downloaded from GitHub. Its keywords are Japanese
  (にこにこ, しくしく ...); TAGS below adds Chinese ones.

Output: one kaomoji per line, "kaomoji<Tab>keywords" (space separated), most
popular first. The engine shows the ones whose keywords appear in what is
being typed (ime/engine/src/kaomoji.rs).
"""
import os
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "ime", "engine", "data", "kaomoji.tsv")
MOZC_URL = "https://raw.githubusercontent.com/google/mozc/master/src/data/emoticon/"

# Mozc's Japanese keywords (and categories) -> Chinese keywords.
TAGS = {
    "にこにこ": "笑 微笑 开心", "にこ": "笑 微笑", "にこっ": "微笑", "すまいる": "微笑",
    "わーい": "开心 好耶 耶", "ばんざい": "万岁 好耶", "ばんざーい": "万岁", "うれしい": "开心 高兴",
    "あせ": "汗 尴尬", "ぎくっ": "尴尬 心虚", "ぽりぽり": "尴尬 挠头", "だらー": "汗",
    "なみだ": "哭 泪目", "なく": "哭 难过", "しくしく": "哭 呜呜", "えーん": "哭 呜呜",
    "うるうる": "泪目 感动", "うわーん": "大哭", "かなしい": "难过 伤心",
    "びっくり": "惊讶 吃惊", "めがてん": "惊讶", "びくっ": "吓", "うおっ": "哇", "どきっ": "心动",
    "さようなら": "再见", "さよなら": "再见", "ばいばい": "拜拜 再见", "またね": "回头见 拜拜",
    "ではでは": "再见", "じとっ": "无语", "じとー": "无语", "じと": "无语", "じろ": "盯",
    "むか": "生气", "むかー": "生气", "むかっ": "生气", "むっ": "不爽", "むきー": "气死",
    "ぷんぷん": "生气 哼", "いかり": "愤怒", "おこる": "生气", "きー": "抓狂",
    "てれ": "害羞", "はずかしい": "害羞", "ぺこり": "鞠躬 谢谢", "おじぎ": "鞠躬",
    "ごめん": "对不起 抱歉", "ごめんなさい": "对不起 抱歉", "うーん": "嗯 思考",
    "えっと": "嗯", "えーと": "嗯", "はーい": "好的 你好", "おーい": "喂 你好",
    "こんにちは": "你好", "ほへー": "发呆", "ぽかーん": "发呆 懵", "ぼうぜん": "懵",
    "しーん": "安静", "ぶい": "耶", "ぴーす": "耶", "どうぶつ": "动物",
    "しょぼーん": "失望 郁闷", "しょんぼり": "失落", "がっくり": "失望", "がくーり": "失望",
    "がっかり": "失望", "はーと": "爱 喜欢", "きりっ": "认真 帅", "きり": "认真",
    "しゃきーん": "精神", "しんけん": "认真", "びしっ": "敬礼", "けいれい": "敬礼",
    "てつやあけ": "熬夜", "てつや": "熬夜", "ねむい": "困", "ねてる": "睡觉",
    "いねむり": "打瞌睡", "ふてね": "睡觉", "ねる": "睡觉", "うぃんく": "眨眼",
    "ういんく": "眨眼", "ぺろ": "调皮", "ぺろっ": "调皮", "したぺろり": "调皮",
    "なぜ": "为什么", "なぞ": "疑问", "はてな": "疑问", "おいしい": "好吃 美味",
    "じゅるっ": "馋", "よだれ": "馋", "はぁ": "唉 叹气", "ためいき": "叹气",
    "おわた": "完蛋 完了", "ありがとう": "谢谢", "ねこ": "猫", "にゃー": "喵 猫",
    "くま": "熊", "くまー": "熊", "よし": "好 棒", "ぐっ": "棒 赞", "いいね": "点赞 赞",
    "いい": "好", "ちゅっ": "亲亲", "くちびる": "亲亲", "だっしゅ": "跑",
    "ぐったり": "累", "げっそり": "累", "きらきら": "闪亮 星星", "ほし": "星星",
    "ながれぼし": "流星", "うっとり": "陶醉", "にやり": "坏笑 得意", "さけび": "喊",
    "たばこ": "抽烟", "いっぷく": "休息", "よしよし": "摸摸 安慰", "あーん": "喂",
    "みんな": "大家", "とんとん": "敲敲", "かもーん": "来", "ばくだん": "炸弹",
    "SMILE": "笑", "SADNESS": "难过", "SWEAT": "汗", "DISPLEASURE": "不爽",
    "SURPRISE": "惊讶",
}


def read(name, local):
    if local:
        with open(os.path.join(local, name), encoding="utf-8") as f:
            return f.read()
    with urllib.request.urlopen(MOZC_URL + name, timeout=60) as r:
        return r.read().decode("utf-8")


def main():
    local = sys.argv[1] if len(sys.argv) > 1 else None
    faces = {}  # kaomoji -> keywords in order (dict: ordered set)

    def add(face, words):
        face = face.strip()
        if not face:
            return
        tags = faces.setdefault(face, {})
        for w in words:
            w = w.strip()
            if w:
                tags[w] = None
                for zh in TAGS.get(w, "").split():
                    tags[zh] = None

    with open(os.path.join(HERE, "popular.tsv"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            cols = line.rstrip("\n").split("\t")
            add(cols[0], " ".join(cols[1:]).split())
    # categorized.tsv: kaomoji, category, keywords; emoticon.tsv: kaomoji,
    # keywords, categories.
    for name, keys, cats in (("categorized.tsv", 2, 1), ("emoticon.tsv", 1, 2)):
        for line in read(name, local).splitlines():
            if line.startswith("#") or line.startswith("\t") or not line.strip():
                continue
            cols = line.split("\t")
            words = (cols[keys].split() if len(cols) > keys else []) + (cols[cats].split() if len(cols) > cats else [])
            add(cols[0], words)

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("# 由 tools/ime-kaomoji/mkkaomoji.py 生成，不要手改。颜文字<Tab>关键词，常用的在前。\n")
        for face, tags in faces.items():
            # Category names are only there to lead to Chinese keywords.
            words = [t for t in tags if t not in TAGS or not t.isupper()]
            f.write(face + "\t" + " ".join(words) + "\n")
    print(len(faces), "kaomoji ->", os.path.normpath(OUT))


main()
