# Shared Unicode character properties

Perex implements Unicode-mode `\p{...}` and `\P{...}` for general categories, the 53 ECMAScript binary character properties, Script and Script_Extensions, including their exact permitted aliases. Property terms can occur alone or in mixed/negated character classes. The implementation uses the existing compiler, class instructions and evaluator. Unicode sets and string properties under `v` remain separate unfinished grammar work.

`tools/unicode-properties.json` records the required binary names/aliases as factual interface data, with the selected ECMA-262 source revision, URL and table hash. UCD 17.0.0 data and source receipts live in `third_party/unicode/17.0.0`, under the Unicode License V3. `tools/generate-properties.py --check` checks source hashes and regenerates exact tables/catalog without host Unicode-library dependencies. The general-category partition is reconstructed for all 1,114,112 code point values. Script_Extensions overrides the default Script membership on the specified ranges; simply adding extension scripts to the old membership would incorrectly retain Common/Inherited values.

The property tables represent 443 properties: 38 general categories/groups, 53 binary properties, and 176 values each for Script and Script_Extensions. The count includes the historical empty `Hrkt` value. General-category queries use masks over one packed partition. Other properties use sorted shared interval boundaries; identical sets share storage. ASCII uses a bit test in the property descriptor and does not search the large Unicode interval table. Aliases use integer offsets into one ASCII name buffer, avoiding a relocatable native pointer for each name.

| Generated table payload | Bytes |
|---|---:|
| General-category partition | 16,576 |
| Property descriptors and ASCII masks | 10,632 |
| Shared interval boundaries | 118,056 |
| Alias indexes | 4,832 |
| Alias text | 3,707 |
| Total property payload | 153,803 |

These bytes are immutable native data shared by every compiled program, with no initialization, writable cache or allocation. The number excludes code/slice descriptors and is not a measured RSS delta. Unused pages need not be accessed by an ASCII query. Unicode case-equivalence tables remain a separately recorded 9,392-byte payload.

Program format 3 permits a class-table record to be either a literal interval or a property ID plus a Boolean complement bit. An isolated `\p{...}` program is 84 bytes irrespective of the property's number of Unicode intervals. Property IDs, flags and Unicode mode are validated when the program is borrowed; changing Unicode data/IDs requires explicit format compatibility handling. Programs can move independently of the shared tables, and never copy a property table into their own storage.

Membership queries read the cursor's current scalar value. They do not allocate, fold, normalize, copy or convert the subject. In `u` ignore-case mode the evaluator tests original property/complement membership across case equivalents. Complementing after folding would give the wrong answer for patterns such as `\P{Lowercase_Letter}`. `v` has different set-order rules and is not silently treated as `u`.

## Reference discrepancy: the empty historical script

The selected [ECMAScript rules](https://tc39.es/ecma262/multipage/text-processing.html#sec-runtime-semantics-unicodematchpropertyvalue-p-v) require the values and aliases listed in PropertyValueAliases.txt. That file includes `Hrkt` / `Katakana_Or_Hiragana`; [Unicode explicitly describes this value as empty](https://www.unicode.org/reports/tr44/#Property_Values). Perex accepts it as an empty set for Script and Script_Extensions. Node 26.8.1 rejects these constructors; pinned QuickJS accepts them and agrees with the empty membership. This discrepancy must remain visible in strict comparisons, with exact inputs and both answers retained. It is not a reason to remove a specified value or to claim complete Node parity.

## Verification boundary

`property_probe` compiles each property through the public compiler and executes the real VM on all 1,114,112 original UTF-16 subjects, including lone surrogate values and astral pairs. Every successful result also checks original capture span/units. `check-properties.mjs` independently scans Node's accepted properties, compares their complete membership boundaries and checks complete answers for aliases, complements, case folding, mixed classes, assertions/backreferences and invalid syntax. Constructor disagreements are distinct from membership disagreement. This is property-grammar and membership evidence, not full ECMAScript conformance, host integration or performance acceptance.

The completed run covers 493,551,616 Perex membership values. Node accepts 441 properties, covering 491,323,392 values, all with identical membership. Its two rejected `Hrkt` constructors are preserved separately. The full-answer matrix has 262,958 cases: 262,350 exact Node agreements and 608 answers for those same empty-script aliases. Pinned QuickJS independently agrees with Perex on all 608. Development CI enables an exact input/answer list in `tests/fixtures/property-reference-disagreements.json`; any new or changed discrepancy fails. Default strict mode reports all 610 differences (two constructors plus 608 full answers). These records are not adoption exclusions.
