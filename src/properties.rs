//! Shared immutable Unicode membership data; programs store only property IDs.
#[path = "property_data.rs"]
mod data;

pub(crate) fn valid(id: u32) -> bool {
    (id as usize) < data::PROPERTIES.len()
}

/// ASCII stays in the small descriptor table; non-ASCII membership uses a
/// category mask or binary search over shared, disjoint interval boundaries.
pub(crate) fn contains(id: u32, c: u32) -> bool {
    let row = data::PROPERTIES[id as usize];
    if c < 128 {
        return row[2 + c as usize / 32] & (1 << (c % 32)) != 0;
    }
    if row[0] & 0x80000000 != 0 {
        let at = data::CATEGORY.partition_point(|&value| value >> 5 <= c);
        row[1] & (1 << (data::CATEGORY[at - 1] & 31)) != 0
    } else {
        let boundaries = &data::BOUNDS[row[0] as usize..(row[0] + row[1]) as usize];
        boundaries.partition_point(|&point| point <= c) % 2 != 0
    }
}

fn lookup(table: &[[u32; 2]], name: &[u8]) -> Option<u32> {
    table
        .binary_search_by(|row| {
            let start = (row[0] & 0xffffff) as usize;
            let length = (row[0] >> 24) as usize;
            data::NAMES[start..start + length].cmp(name)
        })
        .ok()
        .map(|at| table[at][1])
}

pub(crate) fn resolve(name: &[u8], value: Option<&[u8]>) -> Option<u32> {
    let Some(value) = value else {
        return lookup(data::LONE, name);
    };
    match name {
        b"gc" | b"General_Category" => lookup(data::GENERAL_CATEGORY, value),
        b"sc" | b"Script" => lookup(data::SCRIPT, value),
        b"scx" | b"Script_Extensions" => {
            lookup(data::SCRIPT, value).map(|id| id + data::EXTENSION_OFFSET)
        }
        _ => None,
    }
}
