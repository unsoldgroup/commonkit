const MARKETING = [
  "seamless", "seamlessly", "robust", "powerful", "cutting-edge",
  "effortless", "effortlessly", "world-class", "next-generation",
  "revolutionary", "blazing", "lightning-fast", "elegant", "delightful",
  "turnkey", "best-in-class", "state-of-the-art", "game-changing",
  "first-class", "battle-tested", "enterprise-grade", "supercharge",
  "unlock", "unleash", "empower", "empowers",
];
const BANNED = [
  "begin", "begins", "commence", "commences", "initiate", "initiates",
  "originate", "utilize", "utilizes", "utilizing", "leverage", "leverages",
  "leveraging", "facilitate", "facilitates", "ensure", "ensures",
  "ensuring", "prior to", "subsequent to", "obtain", "obtains", "acquire",
  "acquires", "demonstrate", "demonstrates", "additionally", "furthermore",
  "moreover", "comprehensive", "comprehensively", "utilization",
  "aforementioned", "henceforth", "therein", "whilst", "amongst",
  "numerous", "myriad", "plethora", "in order to", "a variety of",
  "in the event that", "due to the fact that", "it is important to note",
];
const PHRASAL = [
  "spin up", "spin down", "reach out", "dive into", "dives into",
  "diving into", "kick off", "kicks off", "roll out", "rolls out",
  "tear down", "ramp up", "circle back", "drill down", "spun up",
  "reaching out",
];
const MODAL_HEDGE = [
  "it is important to note", "it should be noted", "it is worth noting",
  "please note that", "as mentioned", "as noted above",
];

const escapeRegex = (value) =>
  value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export function stripCode(text) {
  return text.replace(/```[\s\S]*?```/g, " ").replace(/`[^`]*`/g, " ");
}

function sentences(text) {
  const output = [];
  for (const line of text.split("\n")) {
    let value = line.trim();
    if (!value) continue;
    value = value
      .replace(/^\s*#{1,6}\s*/, "")
      .replace(/^\s*(?:[-*+]|\d+[.)])\s+/, "");
    if (!value) continue;
    output.push(
      ...value
        .split(/(?<=[.!?:])\s+(?=[A-Z0-9"'\-])/)
        .map((part) => part.trim())
        .filter(Boolean),
    );
  }
  return output;
}

function wordCount(text) {
  return text.match(/[A-Za-z0-9][A-Za-z0-9'\-/]*/g)?.length ?? 0;
}

function countPhrases(text, phrases) {
  const lower = text.toLowerCase();
  const hits = [];
  for (const phrase of phrases) {
    const matcher = new RegExp(
      `(?<![a-z])${escapeRegex(phrase)}(?![a-z])`,
      "g",
    );
    for (const _match of lower.matchAll(matcher)) hits.push(phrase);
  }
  return { count: hits.length, hits };
}

export function scoreWritingForm(raw) {
  const text = stripCode(raw);
  const sentenceList = sentences(text);
  const words =
    sentenceList.reduce((total, sentence) => total + wordCount(sentence), 0) ||
    1;
  const longSentences = sentenceList
    .map((sentence) => [wordCount(sentence), sentence])
    .filter(([count]) => count > 20);
  const banned = countPhrases(text, BANNED);
  const marketing = countPhrases(text, MARKETING);
  const violations = {
    "long_sentence(>20w)": longSentences.length,
    semicolon: [...text.matchAll(/;/g)].length,
    contraction: [...text.matchAll(/\b\w+['’](?:t|re|ve|ll|d|s|m)\b/g)].length,
    passive_voice: [
      ...text.matchAll(
        /\b(?:am|is|are|was|were|be|been|being)\s+(?:\w+ed|done|made|sent|read|built|kept|held|set|put|run|written|shown|given|taken|found|got|gotten|seen|known|thrown|drawn)\b/gi,
      ),
    ].length,
    ing_main_verb: [
      ...text.matchAll(/\b(?:am|is|are|was|were|be|been|being)\s+\w+ing\b/gi),
    ].length,
    nominalization:
      [
        ...text.matchAll(
          /\b(?:perform(?:s|ed)?|conduct(?:s|ed)?|provide(?:s|d)?|carry out|carries out|make use of|makes use of)\b/gi,
        ),
      ].length +
      [...text.matchAll(/\b\w{4,}(?:tion|ment|ance|ence)\s+of\b/gi)].length,
    phrasal_verb: countPhrases(text, PHRASAL).count,
    banned_word: banned.count,
    marketing_adjective: marketing.count,
    modal_hedge: countPhrases(text, MODAL_HEDGE).count,
    "long_paragraph(>6s)": raw
      .split(/\n\s*\n/)
      .filter((paragraph) => paragraph.trim())
      .filter((paragraph) => sentences(stripCode(paragraph)).length > 6).length,
  };
  const total = Object.values(violations).reduce(
    (sum, count) => sum + count,
    0,
  );
  return {
    metric: "writing-form violations per 100 words",
    words,
    sentences: sentenceList.length,
    violations,
    total,
    totalPer100Words: Math.round((total * 10000) / words) / 100,
    emDashes: [...raw.matchAll(/[—–]/g)].length,
    longestSentenceWords: Math.max(
      0,
      ...sentenceList.map((sentence) => wordCount(sentence)),
    ),
    sampleMarketing: [...new Set(marketing.hits)].slice(0, 6),
    sampleBanned: [...new Set(banned.hits)].slice(0, 6),
  };
}
