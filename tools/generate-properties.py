#!/usr/bin/env python3
"""Pinned ECMAScript Unicode character properties, shared by every program."""
import argparse
import collections
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / 'third_party/unicode/17.0.0'
LIMIT = 0x110000


def fields(name):
    for line in (DATA / name).read_text().splitlines():
        line = line.split('#', 1)[0].strip()
        if line:
            yield [p.strip() for p in line.split(';')]


def interval(text):
    ends = text.split('..')
    return int(ends[0], 16), int(ends[-1], 16) + 1


def merged(ranges):
    result = []
    for lo, hi in sorted(ranges):
        assert 0 <= lo < hi <= LIMIT
        if result and lo <= result[-1][1]:
            result[-1] = (result[-1][0], max(hi, result[-1][1]))
        else:
            result.append((lo, hi))
    return result


def minus(left, right):
    result = []
    j = 0
    for lo, hi in left:
        while j < len(right) and right[j][1] <= lo:
            j += 1
        k = j
        while k < len(right) and right[k][0] < hi:
            a, b = right[k]
            if lo < a:
                result.append((lo, min(a, hi)))
            lo = max(lo, b)
            k += 1
        if lo < hi:
            result.append((lo, hi))
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    receipt = json.loads((DATA / 'receipt.json').read_text())
    for f in receipt['files']:
        value = (DATA / f['path']).read_bytes()
        assert len(value) == f['bytes'] and hashlib.sha256(value).hexdigest() == f['sha256'], f['path']
    standard = json.loads((ROOT / 'tools/unicode-properties.json').read_text())
    ucd = list(fields('UnicodeData.txt'))
    category_names = sorted({f[2] for f in ucd} | {'Cn'})
    assert len(category_names) == 30
    category_ids = {name: i for i, name in enumerate(category_names)}
    categories = bytearray([category_ids['Cn']]) * LIMIT
    first = None
    mirrored = []
    for f in ucd:
        c = int(f[0], 16)
        if f[1].endswith(', First>'):
            assert first is None
            first = (c, f[2])
        elif f[1].endswith(', Last>'):
            assert first is not None and first[1] == f[2]
            categories[first[0]:c + 1] = bytes([category_ids[f[2]]]) * (c + 1 - first[0])
            first = None
        else:
            categories[c] = category_ids[f[2]]
        if f[9] == 'Y':
            mirrored.append((c, c + 1))
    assert first is None
    category_runs = [(c << 5) | value for c, value in enumerate(categories)
                     if c == 0 or value != categories[c - 1]]
    restored = bytearray(LIMIT)
    for i, row in enumerate(category_runs):
        lo, value = row >> 5, row & 31
        hi = category_runs[i + 1] >> 5 if i + 1 < len(category_runs) else LIMIT
        restored[lo:hi] = bytes([value]) * (hi - lo)
    assert restored == categories

    binary = collections.defaultdict(list)
    for name in ['DerivedCoreProperties.txt','PropList.txt','DerivedNormalizationProps.txt','emoji/emoji-data.txt']:
        for f in fields(name):
            if f[1] in standard['binary']:
                binary[f[1]].append(interval(f[0]))
    binary['ASCII'] = [(0, 128)]
    binary['Any'] = [(0, LIMIT)]
    binary['Bidi_Mirrored'] = mirrored
    assert set(standard['binary']) == set(binary) | {'Assigned'}
    for key in binary:
        binary[key] = merged(binary[key])
    # Ensure only the specification's binary aliases are exposed, even when UCD
    # adds properties that ECMAScript does not admit.
    ucd_aliases = {f[1]: set(f) for f in fields('PropertyAliases.txt')}
    for key, aliases in standard['binary'].items():
        if key not in ('ASCII','Any','Assigned'):
            assert set(aliases) <= ucd_aliases[key], key

    gc_aliases = {}
    script_aliases = {}
    for f in fields('PropertyValueAliases.txt'):
        if f[0] == 'gc':
            for alias in f[1:]:
                if alias:
                    gc_aliases[alias] = f[1]
        elif f[0] == 'sc':
            for alias in f[1:]:
                if alias:
                    script_aliases[alias] = f[1]
    script_names = sorted(set(script_aliases.values()))
    scripts = {name: [] for name in script_names}
    for f in fields('Scripts.txt'):
        scripts[script_aliases[f[1]]].append(interval(f[0]))
    for key in scripts:
        scripts[key] = merged(scripts[key])
    assert not scripts['Zzzz']
    scripts['Zzzz'] = minus([(0, LIMIT)], merged([r for ranges in scripts.values() for r in ranges]))
    overrides = []
    additions = collections.defaultdict(list)
    for f in fields('ScriptExtensions.txt'):
        span = interval(f[0])
        overrides.append(span)
        for script in f[1].split():
            additions[script_aliases[script]].append(span)
    overrides = merged(overrides)
    extensions = {name: merged(minus(scripts[name], overrides) + additions[name]) for name in script_names}

    bounds = []
    descriptors = []
    shared = {}
    names = {}
    catalog = []

    def add(name, ranges=None, mask=None, aliases=()):
        index = len(descriptors)
        names[name] = index
        if mask is not None:
            a, b = 0x80000000, mask
            ascii_bits = [sum(1 << (c % 32) for c in range(word*32, word*32+32)
                              if mask & (1 << categories[c])) for word in range(4)]
        else:
            flat = tuple(value for span in ranges for value in span)
            if flat not in shared:
                shared[flat] = len(bounds)
                bounds.extend(flat)
            a, b = shared[flat], len(flat)
            ascii_bits = [sum(1 << (c % 32) for c in range(word*32, word*32+32)
                              if any(lo <= c < hi for lo, hi in ranges)) for word in range(4)]
        descriptors.append((a,b,*ascii_bits))
        catalog.append(dict(name=name,aliases=sorted(set(aliases))))
        return index

    gc_ids = {}
    for name in sorted(set(gc_aliases.values())):
        parts = [n for n in category_names if n.startswith(name)] if len(name) == 1 else [name]
        if name == 'LC':
            parts = ['Lu','Ll','Lt']
        mask = sum(1 << category_ids[n] for n in parts)
        assert mask
        aliases = [alias for alias, value in gc_aliases.items() if value == name]
        gc_ids[name] = add('General_Category='+name,mask=mask,
                           aliases=aliases+['gc='+a for a in aliases]+['General_Category='+a for a in aliases])
    lone = {alias: gc_ids[value] for alias, value in gc_aliases.items()}
    for name, aliases in sorted(standard['binary'].items()):
        kwargs = dict(mask=((1 << 30)-1) ^ (1 << category_ids['Cn'])) if name == 'Assigned' else dict(ranges=binary[name])
        index = add(name,aliases=aliases,**kwargs)
        for alias in aliases:
            assert alias not in lone
            lone[alias] = index
    script_indices = {}
    for name in script_names:
        aliases = [alias for alias, value in script_aliases.items() if value == name]
        index = add('Script='+name,ranges=scripts[name],aliases=['sc='+a for a in aliases]+['Script='+a for a in aliases])
        script_indices[name] = index
    # The offset is explicit; alias ids are not assumed to reflect enum order.
    extension_offset = len(descriptors) - min(script_indices.values())
    for name in script_names:
        aliases = [alias for alias, value in script_aliases.items() if value == name]
        index = add('Script_Extensions='+name,ranges=extensions[name],
                    aliases=['scx='+a for a in aliases]+['Script_Extensions='+a for a in aliases])
        assert index == script_indices[name] + extension_offset

    # Properties of strings (UTS #51). Their members of one code point are
    # exactly Emoji_Presentation without the regional indicators, which the
    # engine spells with the property references it already has; only the
    # members of two or more code points need data of their own.
    sequence_names = ['Basic_Emoji','Emoji_Keycap_Sequence','RGI_Emoji','RGI_Emoji_Flag_Sequence',
                      'RGI_Emoji_Modifier_Sequence','RGI_Emoji_Tag_Sequence','RGI_Emoji_ZWJ_Sequence']
    sequences = collections.defaultdict(list)
    emoji_singles = []
    for source in ['emoji/emoji-sequences.txt','emoji/emoji-zwj-sequences.txt']:
        for f in fields(source):
            assert f[1] in sequence_names and f[1] != 'RGI_Emoji', f[1]
            codes = f[0].split()
            if len(codes) == 1:
                assert f[1] == 'Basic_Emoji'
                emoji_singles.append(interval(codes[0]))
            else:
                sequences[f[1]].append(tuple(int(c, 16) for c in codes))
    assert merged(emoji_singles) == minus(binary['Emoji_Presentation'], binary['Regional_Indicator'])
    sequences['RGI_Emoji'] = [s for name in sequence_names if name != 'RGI_Emoji' for s in sequences[name]]
    # A `v` set is closed under simple case folding before it is matched, and
    # one sequence member is cased (U+24C2, whose sequence is `\p{Basic_Emoji}`'s
    # circled M), so the evaluator compares members under folding rather than
    # exactly. The count is reported so that a Unicode update which changes it
    # is visible.
    cased = set()
    for f in fields('CaseFolding.txt'):
        if f[1] in ('C', 'S'):
            cased.add(int(f[0], 16))
            cased.add(int(f[2], 16))
    cased_members = {c for members in sequences.values() for s in members for c in s} & cased

    points = []
    offsets = {}
    sequence_index = []
    groups = []
    sets = []
    for name in sequence_names:
        members = sorted(set(sequences[name]))
        assert len(members) == len(sequences[name]), name
        group_start, index_start = len(groups), len(sequence_index)
        for length in sorted({len(s) for s in members}, reverse=True):
            start = len(sequence_index)
            for member in sorted(s for s in members if len(s) == length):
                if member not in offsets:
                    offsets[member] = len(points)
                    points.extend(member)
                # The length rides in the index entry's top bits, so a member
                # is read without finding the group it belongs to first.
                assert offsets[member] < (1 << 20) and length < (1 << 6)
                sequence_index.append(offsets[member] | (length << 20))
            groups.append((length, start, len(sequence_index) - start))
        # A subject character that begins no member of the set rejects it
        # without searching: the ASCII ones are a bitmap, and the rest are one
        # flag, because every set has members beginning above ASCII.
        firsts = {s[0] for s in members}
        ascii_bits = [sum(1 << (c % 32) for c in firsts if word*32 <= c < word*32+32)
                      for word in range(4)]
        sets.append((group_start, len(groups) - group_start, index_start,
                     len(sequence_index) - index_start,
                     # `Basic_Emoji` is the only set with members of one code
                     # point; `RGI_Emoji` holds them through it.
                     int(name in ('Basic_Emoji', 'RGI_Emoji')),
                     *ascii_bits, int(any(c >= 128 for c in firsts))))
    sequence_report = dict(sets=len(sets), sequences=len(offsets), cased_members=len(cased_members),
                           point_bytes=len(points)*4, index_bytes=len(sequence_index)*4,
                           group_bytes=len(groups)*12, set_bytes=len(sets)*40)

    strings = bytearray()
    interned = {}
    output = ['// Generated by tools/generate-properties.py; do not edit.',
              '// Unicode 17.0.0, Unicode License V3; see third_party/unicode/17.0.0.',
              f'pub(super) const EXTENSION_OFFSET: u32 = {extension_offset};',
              f'pub(super) const ID_START: u32 = {names["ID_Start"]};',
              f'pub(super) const ID_CONTINUE: u32 = {names["ID_Continue"]};']

    def array(name, rows, width=None):
        output.append('#[rustfmt::skip]')
        if width:
            output.append(f'pub(super) static {name}: &[[u32; {width}]] = &[')
            output.extend('    ['+', '.join(f'0x{n:x}' for n in row)+'],' for row in rows)
        else:
            output.append(f'pub(super) static {name}: &[u32] = &[')
            output.extend('    '+', '.join(f'0x{n:x}' for n in rows[i:i+12])+',' for i in range(0,len(rows),12))
        output.append('];')

    array('PROPERTIES', descriptors, 6)
    array('CATEGORY', category_runs)
    array('BOUNDS', bounds)
    alias_bytes = 0
    for name, aliases in [('LONE',lone),('GENERAL_CATEGORY',{a:gc_ids[v] for a,v in gc_aliases.items()}),
                          ('SCRIPT',{a:script_indices[v] for a,v in script_aliases.items()})]:
        rows = []
        for alias,index in sorted(aliases.items()):
            value = alias.encode('ascii')
            assert len(value) <= 64
            if alias not in interned:
                interned[alias] = len(strings)
                strings.extend(value)
            assert interned[alias] < 1 << 24
            rows.append((interned[alias] | (len(value) << 24),index))
        array(name,rows,2)
        alias_bytes += len(rows)*8
    output.append(f'pub(super) static NAMES: &[u8] = b"{strings.decode()}";')
    generated = '\n'.join(output)+'\n'

    sequence_output = ['// Generated by tools/generate-properties.py; do not edit.',
                       '// Unicode 17.0.0 with Emoji 17.0, Unicode License V3;',
                       '// see third_party/unicode/17.0.0.',
                       '/// The properties of strings, in the order their ids are assigned.',
                       'pub(super) static SEQUENCE_NAMES: &[&str] = &[',
                       *[f'    "{name}",' for name in sequence_names], '];',
                       '/// Code points of the members that are not one code point: the single',
                       "/// code point members of `Basic_Emoji` are `Emoji_Presentation` without",
                       '/// the regional indicators, which needs no table of its own.',
                       f'pub(super) const EMOJI_PRESENTATION: u32 = {names["Emoji_Presentation"]};',
                       f'pub(super) const REGIONAL_INDICATOR: u32 = {names["Regional_Indicator"]};',
                       '/// Code points in the longest member of any set.',
                       f'pub(super) const LONGEST: usize = {max(len(s) for m in sequences.values() for s in m)};']
    output = sequence_output
    array('SEQUENCE_SETS', sets, 10)
    array('SEQUENCE_GROUPS', groups, 3)
    array('SEQUENCE_INDEX', sequence_index)
    array('SEQUENCE_POINTS', points)
    sequence_generated = '\n'.join(output)+'\n'
    report = dict(unicode='17.0.0',properties=len(descriptors),binary=len(standard['binary']),
                  general_categories=len(gc_ids),scripts=len(script_names),category_bytes=len(category_runs)*4,
                  descriptor_bytes=len(descriptors)*24,boundary_bytes=len(bounds)*4,alias_bytes=alias_bytes,
                  name_bytes=len(strings),shared_range_sets=len(shared),verified_category_values=LIMIT)
    report['table_payload_bytes'] = sum(report[k] for k in ['category_bytes','descriptor_bytes','boundary_bytes','alias_bytes','name_bytes'])
    report['sequences'] = sequence_report
    report['table_payload_bytes'] += sum(sequence_report[k] for k in
                                         ['point_bytes','index_bytes','group_bytes','set_bytes'])
    for path, text in [(ROOT/'src/property_data.rs',generated),
                       (ROOT/'src/sequence_data.rs',sequence_generated),
                       (ROOT/'tests/fixtures/properties.json',json.dumps(dict(report=report,properties=catalog),indent=2)+'\n')]:
        if args.check:
            assert path.read_text() == text, path
        else:
            path.write_text(text)
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
