use crate::runtime::task;

pub(crate) trait Overflow<T: 'static> {
    fn push(&self, task: task::NotifiedTask<T>);

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item = task::NotifiedTask<T>>;
}
