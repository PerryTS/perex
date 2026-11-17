//! Shared immutable Unicode membership data; programs store only property IDs.
#[path = "property_data.rs"]
mod data;

#[cfg(feature = "selectable-unicode")]
static MEMBERSHIP: std::sync::OnceLock<Membership> = std::sync::OnceLock::new();

/// Install the immutable Unicode membership provider before compiling or
/// validating programs that need properties or non-ASCII capture names.
///
/// Installation is idempotent and safe across threads. The opt-in
/// `selectable-unicode` feature requires std; an absent provider panics on the
/// cold compile/validation path. Validated programs retain the resolved native
/// membership function and DFA entries, so matching and resumed borrows never
/// read the installation slot.
/// Without this feature the engine remains no_std and calls the database
/// directly; installation is a no-op.
///
/// Hosts accepting arbitrary patterns or imported programs must install
/// conservatively. Omitting every reference to this function lets linker
/// section collection discard the membership tables. Names, descriptors,
/// case-fold data and properties of strings are independent.
pub fn install_unicode_properties() {
    #[cfg(feature = "selectable-unicode")]
    let _ = MEMBERSHIP.set(Membership {
        dfa: &crate::dfa::UNICODE_DISPATCH,
    });
}

/// A process-local capability, never part of relocatable program words.
/// The default configuration is zero-sized and retains direct dispatch.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Membership {
    #[cfg(feature = "selectable-unicode")]
    pub(crate) dfa: &'static crate::dfa::Dispatch,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Query(#[cfg(feature = "selectable-unicode")] fn(u32, u32) -> bool);

#[cfg(feature = "selectable-unicode")]
impl Query {
    pub(crate) const fn unicode() -> Self {
        Self(contains_data)
    }
    pub(crate) const fn property_free() -> Self {
        // Only property-free programs carry this function. Compilation and
        // validation resolve every property before publishing a program.
        Self(|_, _| unreachable!("property-free program queried membership"))
    }
}

impl PartialEq for Membership {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(feature = "selectable-unicode")]
        {
            core::ptr::eq(self.dfa, other.dfa)
        }
        #[cfg(not(feature = "selectable-unicode"))]
        {
            let _ = other;
            true
        }
    }
}
impl Eq for Membership {}

impl Membership {
    #[inline(always)]
    pub(crate) fn query(self) -> Query {
        #[cfg(feature = "selectable-unicode")]
        return self.dfa.query;
        #[cfg(not(feature = "selectable-unicode"))]
        Query()
    }
    #[inline(always)]
    pub(crate) fn contains(self, id: u32, c: u32) -> bool {
        self.query().contains(id, c)
    }
}

impl Query {
    #[inline(always)]
    pub(crate) fn contains(self, id: u32, c: u32) -> bool {
        #[cfg(feature = "selectable-unicode")]
        {
            (self.0)(id, c)
        }
        #[cfg(not(feature = "selectable-unicode"))]
        contains_data(id, c)
    }
}

/// A DFA instantiation fixes membership at compile time; dispatch is outside
/// its character and state-building loops.
pub(crate) trait Provider {
    fn contains(id: u32, point: u32) -> bool;
}

pub(crate) struct Unicode;
impl Provider for Unicode {
    #[inline(always)]
    fn contains(id: u32, point: u32) -> bool {
        contains_data(id, point)
    }
}

#[cfg(feature = "selectable-unicode")]
pub(crate) struct PropertyFree;
#[cfg(feature = "selectable-unicode")]
impl Provider for PropertyFree {
    #[inline(always)]
    fn contains(_: u32, _: u32) -> bool {
        unreachable!("validated property-free program queried membership")
    }
}

/// Compilation/validation alone may discover a first property. Once resolved,
/// the same capability travels through preparation, emission and validation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Resolver {
    #[cfg(feature = "selectable-unicode")]
    membership: Option<Membership>,
}

impl Resolver {
    pub(crate) fn require(&mut self) -> Membership {
        #[cfg(feature = "selectable-unicode")]
        {
            *self.membership.get_or_insert_with(|| {
                #[cfg(test)]
                SLOT_READS.with(|reads| reads.set(reads.get() + 1));
                *MEMBERSHIP
                    .get()
                    .expect("Unicode properties must be installed by the host")
            })
        }
        #[cfg(not(feature = "selectable-unicode"))]
        Membership::default()
    }

    pub(crate) fn get(self) -> Membership {
        #[cfg(feature = "selectable-unicode")]
        {
            self.membership.unwrap_or_default()
        }
        #[cfg(not(feature = "selectable-unicode"))]
        Membership::default()
    }
}

#[cfg(all(test, feature = "selectable-unicode"))]
std::thread_local! {
    static SLOT_READS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

pub(crate) fn identifier(resolver: &mut Resolver, c: u32, first: bool) -> bool {
    if c < 128 {
        return matches!(c, 36 | 65..=90 | 95 | 97..=122) || (!first && matches!(c, 48..=57));
    }
    (!first && matches!(c, 0x200c | 0x200d))
        || resolver.require().contains(
            if first {
                data::ID_START
            } else {
                data::ID_CONTINUE
            },
            c,
        )
}

pub(crate) fn valid(id: u32) -> bool {
    (id as usize) < data::PROPERTIES.len()
}

/// Exact ASCII membership already stored in the shared descriptor table.
pub(crate) fn ascii(id: u32) -> u128 {
    let row = data::PROPERTIES[id as usize];
    u128::from(row[2])
        | (u128::from(row[3]) << 32)
        | (u128::from(row[4]) << 64)
        | (u128::from(row[5]) << 96)
}

/// ASCII stays in the small descriptor table; non-ASCII membership uses a
/// category mask or binary search over shared, disjoint interval boundaries.
#[inline(always)]
fn contains_data(id: u32, c: u32) -> bool {
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

#[cfg(test)]
mod tests;
