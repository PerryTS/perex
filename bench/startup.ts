// Each route is compiled at startup and searched once on a short subject.
const rules: RegExp[] = [];
const suffixes = ["ping", "(?:red|blue)", "[a-z]{1,5}", "[0-9]+", "(ab)+", "x\\s+y", "p(?:q|r)*", "\\p{L}{1,3}"];
const subjects = ["ping", "blue", "abc", "57", "abab", "x y", "prq", "Ω"];
for (let group = 0; group < 56; group++) {
    for (let shape = 0; shape < suffixes.length; shape++) {
        const flags = shape === 7 ? "u" : shape === 1 ? "i" : "";
        rules.push(new RegExp("^r" + group + "/" + suffixes[shape] + "$", flags));
    }
}
let hits = 0;
let matchedUnits = 0;
for (let i = 0; i < rules.length; i++) {
    const subject = (i % 3 === 0 ? "s" : "r") + Math.floor(i / 8) + "/" + subjects[i % 8];
    const match = rules[i].exec(subject);
    if (match !== null) {
        hits++;
        matchedUnits += match[0].length;
    }
}
console.log(rules.length, hits, matchedUnits);
