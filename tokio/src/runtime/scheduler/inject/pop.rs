use super::InjectSyncState;

use crate::runtime::task;

use std::marker::PhantomData;

pub(crate) struct Pop<'a, T: 'static> {
    len: usize,
    injectSyncState: &'a mut InjectSyncState,
    _p: PhantomData<T>,
}

impl<'a, T: 'static> Pop<'a, T> {
    pub(super) fn new(len: usize, injectSyncState: &'a mut InjectSyncState) -> Pop<'a, T> {
        Pop {
            len,
            injectSyncState,
            _p: PhantomData,
        }
    }
}

impl<'a, T: 'static> Iterator for Pop<'a, T> {
    type Item = task::Notified<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        let ret = self.injectSyncState.pop();

        // should be `Some` when `len > 0`
        debug_assert!(ret.is_some());

        self.len -= 1;

        ret
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<'a, T: 'static> ExactSizeIterator for Pop<'a, T> {
    fn len(&self) -> usize {
        self.len
    }
}

impl<'a, T: 'static> Drop for Pop<'a, T> {
    fn drop(&mut self) {
        for _ in self.by_ref() {}
    }
}
