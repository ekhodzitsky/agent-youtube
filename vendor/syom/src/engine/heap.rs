//! Heap-built fixed-size arrays (TASK-118): `Box::new([v; N])` builds the
//! array on the stack first, which for frame-sized state costs tens of KiB
//! of stack in constructors. Going through a `Vec` never does.

/// `Box<[T; N]>` filled with clones of `v`, without an `N`-element stack
/// temporary.
pub(crate) fn heap_array<T: Clone, const N: usize>(v: T) -> Box<[T; N]> {
    match vec![v; N].into_boxed_slice().try_into() {
        Ok(a) => a,
        Err(_) => unreachable!("vec![v; N] always has N elements"),
    }
}
