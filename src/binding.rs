//! Owner-bound validation for immutable storage that may relocate.
//!
//! A binding validates once and retains its owner, never an interior pointer.
//! Fresh views check fixed-size layout metadata. The owner must preserve exact
//! contents: equal lengths or headers are not a proof of immutable identity.
//! These semantic traits never authorize unsafe code in the Perex core.
use crate::{
    Budget,
    executor::Resources,
    input::{EncodingError, Input},
    program::{Program, ProgramError},
};

/// Original subject storage, without conversion or materialization.
#[derive(Clone, Copy, Debug)]
pub enum Subject<'a> {
    Wtf8(&'a [u8]),
    Utf16(&'a [u16]),
}

/// Owner or rooted handle for one immutable logical subject.
///
/// Every successful callback must observe exactly the same bytes/units and
/// representation for this owner's binding lifetime. An owner may move its
/// backing allocation between views, but must preserve its contents and root
/// identity. It must enforce the host's sharing/immutability rules, not merely
/// keep a mutable buffer alive.
///
/// Acquiring/releasing a view must not allocate, collect or invoke host code
/// other than the supplied callback. The callback must not collect or mutate
/// storage while a view is live. This permits program and subject views to be
/// nested without an intervening collecting getter. Allocation, coercion and
/// callbacks belong outside these scopes.
///
/// Violating immutable identity can yield wrong answers or a panic. Layout
/// guards do not detect equal-length mutation; implementations must not rely on
/// them for that purpose. All core access remains checked, safe Rust.
pub trait ImmutableSubject {
    type Error;
    fn with_subject<T>(&self, use_subject: impl FnOnce(Subject<'_>) -> T)
    -> Result<T, Self::Error>;
}

/// Owner or rooted handle for one immutable program allocation.
///
/// The identity, immutability and non-collecting view requirements of
/// [`ImmutableSubject`] apply to every word of the program, including tables.
/// Each slice must have the same contents and length as the initial validated
/// program. Relocation may change its address only. A header check cannot
/// establish that operands or tables have not changed.
pub trait ImmutableProgram {
    type Error;
    fn with_words<T>(&self, use_words: impl FnOnce(&[u32]) -> T) -> Result<T, Self::Error>;
}

impl ImmutableSubject for [u8] {
    type Error = core::convert::Infallible;
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, Self::Error> {
        Ok(f(Subject::Wtf8(self)))
    }
}
impl ImmutableSubject for [u16] {
    type Error = core::convert::Infallible;
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, Self::Error> {
        Ok(f(Subject::Utf16(self)))
    }
}
impl ImmutableProgram for [u32] {
    type Error = core::convert::Infallible;
    fn with_words<T>(&self, f: impl FnOnce(&[u32]) -> T) -> Result<T, Self::Error> {
        Ok(f(self))
    }
}
impl<S: ImmutableSubject + ?Sized> ImmutableSubject for &S {
    type Error = S::Error;
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, Self::Error> {
        S::with_subject(self, f)
    }
}
impl<P: ImmutableProgram + ?Sized> ImmutableProgram for &P {
    type Error = P::Error;
    fn with_words<T>(&self, f: impl FnOnce(&[u32]) -> T) -> Result<T, Self::Error> {
        P::with_words(self, f)
    }
}

#[derive(Debug)]
pub enum SubjectError<E> {
    Resource(E),
    Encoding(EncodingError),
    ChangedLayout,
}
#[derive(Debug)]
pub enum BoundProgramError<E> {
    Resource(E),
    Validation(ProgramError),
    ChangedLayout,
}

/// Failed binding returns the original owner for explicit caller cleanup.
#[derive(Debug)]
pub struct BindingError<S, E> {
    pub storage: S,
    pub error: E,
}

/// A validated owner; no subject slice or movable address is retained.
///
/// Consuming `into_storage` discards the validation binding. There is no API
/// that attaches its metadata to an unrelated slice or replaces the owner.
pub struct BoundSubject<S: ImmutableSubject> {
    storage: S,
    pub(crate) layout: (u8, usize, usize),
}
impl<S: ImmutableSubject> BoundSubject<S> {
    pub fn new(storage: S) -> Result<Self, BindingError<S, SubjectError<S::Error>>> {
        let result = storage
            .with_subject(|subject| match subject {
                Subject::Wtf8(bytes) => Input::wtf8(bytes).map(Input::shape),
                Subject::Utf16(units) => Ok(Input::utf16(units).shape()),
            })
            .map_err(SubjectError::Resource)
            .and_then(|result| result.map_err(SubjectError::Encoding));
        match result {
            Ok(layout) => Ok(Self { storage, layout }),
            Err(error) => Err(BindingError { storage, error }),
        }
    }

    /// Reborrow in constant work without revalidating or recounting the subject.
    /// The callback result cannot retain the fresh view.
    ///
    /// ```compile_fail
    /// use perex::{binding::{BoundSubject, ImmutableSubject}, input::Input};
    /// fn escape<S: ImmutableSubject>(subject: &BoundSubject<S>) -> Input<'_> {
    ///     subject.with_view(|input| input).ok().unwrap()
    /// }
    /// ```
    pub fn with_view<T>(
        &self,
        f: impl FnOnce(Input<'_>) -> T,
    ) -> Result<T, SubjectError<S::Error>> {
        self.storage
            .with_subject(|subject| {
                let input = match subject {
                    Subject::Wtf8(bytes) => Input::reborrow_bytes(bytes, self.layout),
                    Subject::Utf16(units) => {
                        let input = Input::utf16(units);
                        (input.shape() == self.layout).then_some(input)
                    }
                }
                .ok_or(SubjectError::ChangedLayout)?;
                Ok(f(input))
            })
            .map_err(SubjectError::Resource)?
    }

    /// Bind storage the host has already validated and counted, in constant
    /// work.
    ///
    /// [`BoundSubject::new`] decodes every scalar of a byte subject to check it
    /// and to count its UTF-16 units. A host that already knows both, because
    /// its strings are valid when they are made and carry their length, can
    /// skip that pass. That is what lets a search per call, such as a
    /// JavaScript `exec` loop, cost its matching work rather than the whole
    /// string every time, without the host keeping a cache.
    ///
    /// Only what is constant work is checked: UTF-16 storage must have exactly
    /// `utf16_len` units, and byte storage a length that many units could
    /// occupy, at least one byte and at most three for each. Whether the bytes
    /// are generalized UTF-8 holding exactly `utf16_len` units is the host's
    /// to guarantee, as immutability already is. Storage that breaks that can
    /// produce wrong answers or a panic, never unsafety. A failed check returns
    /// the storage with [`SubjectError::ChangedLayout`].
    pub fn new_counted(
        storage: S,
        utf16_len: usize,
    ) -> Result<Self, BindingError<S, SubjectError<S::Error>>> {
        let result = storage
            .with_subject(|subject| match subject {
                Subject::Wtf8(bytes) => {
                    let possible =
                        utf16_len <= bytes.len() && bytes.len() <= utf16_len.saturating_mul(3);
                    // Valid generalized UTF-8 has as many bytes as units exactly
                    // when every unit is ASCII, which is how `Input::wtf8`
                    // chooses the same representation.
                    let kind = u8::from(utf16_len != bytes.len());
                    possible.then_some((kind, bytes.len(), utf16_len))
                }
                Subject::Utf16(units) => {
                    (units.len() == utf16_len).then(|| Input::utf16(units).shape())
                }
            })
            .map_err(SubjectError::Resource)
            .and_then(|layout| layout.ok_or(SubjectError::ChangedLayout));
        match result {
            Ok(layout) => Ok(Self { storage, layout }),
            Err(error) => Err(BindingError { storage, error }),
        }
    }

    pub fn into_storage(self) -> S {
        self.storage
    }
}

/// What validating a program established, as plain data: its length and
/// header. Obtained only from a [`BoundProgram`] that validated it, and small
/// enough for a host to keep beside the program it describes, so later
/// bindings of that same program need not validate it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramWitness {
    header: [u32; crate::program::HEADER],
    words: usize,
}

/// A program validated once against an immutable owner, with no interior view.
pub struct BoundProgram<P: ImmutableProgram> {
    storage: P,
    header: [u32; crate::program::HEADER],
    words: usize,
}
impl<P: ImmutableProgram> BoundProgram<P> {
    pub fn new(
        storage: P,
        budget: &mut Budget,
    ) -> Result<Self, BindingError<P, BoundProgramError<P::Error>>> {
        let result = storage
            .with_words(|words| {
                Program::from_words(words, budget).map(|p| {
                    (
                        p.words[..crate::program::HEADER].try_into().unwrap(),
                        words.len(),
                    )
                })
            })
            .map_err(BoundProgramError::Resource)
            .and_then(|result| result.map_err(BoundProgramError::Validation));
        match result {
            Ok((header, words)) => Ok(Self {
                storage,
                header,
                words,
            }),
            Err(error) => Err(BindingError { storage, error }),
        }
    }

    /// Bind a program that an earlier binding validated, in constant work and
    /// without a budget.
    ///
    /// The program must be the one `witness` came from, unchanged: the same
    /// immutable words, wherever they have since been moved. Its length and
    /// header are checked, exactly as every view already checks them, and a
    /// mismatch returns the storage with [`BoundProgramError::ChangedLayout`].
    /// The rest of the words are the host's to keep identical, as for every
    /// binding; different words behind an equal header can produce wrong
    /// answers or a panic, never unsafety.
    pub fn new_witnessed(
        storage: P,
        witness: ProgramWitness,
    ) -> Result<Self, BindingError<P, BoundProgramError<P::Error>>> {
        let result = storage
            .with_words(|words| {
                words.len() == witness.words
                    && words.get(..witness.header.len()) == Some(&witness.header)
            })
            .map_err(BoundProgramError::Resource)
            .and_then(|same| {
                if same {
                    Ok(())
                } else {
                    Err(BoundProgramError::ChangedLayout)
                }
            });
        match result {
            Ok(()) => Ok(Self {
                storage,
                header: witness.header,
                words: witness.words,
            }),
            Err(error) => Err(BindingError { storage, error }),
        }
    }

    /// What this binding's validation established, for binding the same
    /// program again with [`BoundProgram::new_witnessed`].
    pub fn witness(&self) -> ProgramWitness {
        ProgramWitness {
            header: self.header,
            words: self.words,
        }
    }

    /// Check the fixed-size header/length and scope a fresh program borrow.
    /// Full word identity is the retained owner's contract, not this guard.
    ///
    /// ```compile_fail
    /// use perex::{binding::{BoundProgram, ImmutableProgram}, program::Program};
    /// fn escape<P: ImmutableProgram>(program: &BoundProgram<P>) -> Program<'_> {
    ///     program.with_view(|p| p).ok().unwrap()
    /// }
    /// ```
    pub fn with_view<T>(
        &self,
        f: impl FnOnce(Program<'_>) -> T,
    ) -> Result<T, BoundProgramError<P::Error>> {
        self.storage
            .with_words(|words| {
                if words.len() != self.words || words.get(..self.header.len()) != Some(&self.header)
                {
                    return Err(BoundProgramError::ChangedLayout);
                }
                Ok(f(Program { words }))
            })
            .map_err(BoundProgramError::Resource)?
    }

    pub fn into_storage(self) -> P {
        self.storage
    }
}

#[derive(Debug)]
pub enum PairError<P, S> {
    Program(BoundProgramError<P>),
    Subject(SubjectError<S>),
}

/// Combine independent bindings; a cached program need not be validated again
/// for every subject. The referenced bindings keep their respective owners.
pub struct BoundResources<'a, P: ImmutableProgram, S: ImmutableSubject> {
    pub program: &'a BoundProgram<P>,
    pub subject: &'a BoundSubject<S>,
}
impl<P: ImmutableProgram, S: ImmutableSubject> Resources for BoundResources<'_, P, S> {
    type Error = PairError<P::Error, S::Error>;
    fn with_views<T>(&self, f: impl FnOnce(Program<'_>, Input<'_>) -> T) -> Result<T, Self::Error> {
        self.program
            .with_view(|p| {
                self.subject
                    .with_view(|input| f(p, input))
                    .map_err(PairError::Subject)
            })
            .map_err(PairError::Program)?
    }
}
