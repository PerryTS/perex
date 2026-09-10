# Research and provenance

QuickJS's standalone regex core is promising starting material because it emits compact bytecode and exposes allocation/timeout hooks. In a separate host investigation, QuickJS revision `04be246001599f5995fa2f2d8c91a0f198d3f34c` agreed with Node 26.8.1 on a reconstructed 4,463-pattern × 14-subject corpus and 37 operation-boundary cases. Two additional cases exposed a 254-explicit-capture-group limit. Those are upstream QuickJS results, not Perex results or performance evidence.

The application corpus, raw measurements, source receipts and infrastructure details remain with the host investigation. This repository contains small authored semantic fixtures that can run independently. No upstream implementation has been imported yet.

Useful primary references:

- [QuickJS libregexp source at the evaluated revision](https://github.com/bellard/quickjs/blob/04be246001599f5995fa2f2d8c91a0f198d3f34c/libregexp.c)
- [QuickJS embedding interface](https://github.com/bellard/quickjs/blob/04be246001599f5995fa2f2d8c91a0f198d3f34c/libregexp.h)
- [ECMAScript text processing specification](https://tc39.es/ecma262/multipage/text-processing.html)
- [Test262](https://github.com/tc39/test262)
- [Linear Matching of JavaScript Regular Expressions](https://arxiv.org/abs/2311.17620)
- [Linden's verified semantics and algorithm scope](https://github.com/LindenRegex/Linden)
- [Mozilla's Irregexp embedding](https://hacks.mozilla.org/2020/06/a-new-regexp-engine-in-spidermonkey/)

Reuse requires source pins, license/notice preservation and tests against the resulting implementation. Porting an algorithm does not inherit a proof about its original implementation. Research on linear subsets does not establish that arbitrary full-JS backreference patterns always finish quickly within bounded memory.
