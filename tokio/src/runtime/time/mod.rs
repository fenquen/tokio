// Currently, rust warns when an unsafe fn contains an unsafe {} block. However,
// in the future, this will change to the reverse. For now, suppress this
// warning and generally stick with being explicit about unsafety.
#![allow(unused_unsafe)]
#![cfg_attr(not(feature = "rt"), allow(dead_code))]

//! Time driver.

mod entry;
pub(crate) use entry::TimerEntry;
use entry::{EntryList, TimerHandle, TimerShared, MAX_SAFE_MILLIS_DURATION};

mod handle;
pub(crate) use self::handle::TimeDriverHandle;
use self::wheel::Wheel;

mod source;
pub(crate) use source::TimeSource;

mod wheel;

use crate::loom::sync::atomic::{AtomicBool, Ordering};
use crate::loom::sync::Mutex;
use crate::runtime::driver::{self, DriverHandle, IOHandleEnum, IoStackEnum};
use crate::time::error::Error;
use crate::time::{Clock, Duration};
use crate::util::WakerList;

use crate::loom::sync::atomic::AtomicU64;
use std::fmt;
use std::sync::RwLock;
use std::{num::NonZeroU64, ptr::NonNull};

struct AtomicOptionNonZeroU64(AtomicU64);

// A helper type to store the `next_wake`.
impl AtomicOptionNonZeroU64 {
    fn new(val: Option<NonZeroU64>) -> Self {
        Self(AtomicU64::new(val.map_or(0, NonZeroU64::get)))
    }

    fn store(&self, val: Option<NonZeroU64>) {
        self.0.store(val.map_or(0, NonZeroU64::get), Ordering::Relaxed);
    }

    fn load(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(self.0.load(Ordering::Relaxed))
    }
}

/// Time implementation that drives [`Sleep`][sleep], [`Interval`][interval], and [`Timeout`][timeout].
///
/// A `Driver` instance tracks the state necessary for managing time and
/// notifying the [`Sleep`][sleep] instances once their deadlines are reached.
///
/// It is expected that a single instance manages many individual [`Sleep`][sleep]
/// instances. The `Driver` implementation is thread-safe and, as such, is able
/// to handle callers from across threads.
///
/// After creating the `Driver` instance, the caller must repeatedly call `park`
/// or `park_timeout`. The time driver will perform no work unless `park` or
/// `park_timeout` is called repeatedly.
///
/// The driver has a resolution of one millisecond. Any unit of time that falls
/// between milliseconds are rounded up to the next millisecond.
///
/// When an instance is dropped, any outstanding [`Sleep`][sleep] instance that has not
/// elapsed will be notified with an error. At this point, calling `poll` on the
/// [`Sleep`][sleep] instance will result in panic.
///
/// # Implementation
///
/// The time driver is based on the [paper by Varghese and Lauck][paper].
///
/// A hashed timing wheel is a vector of slots, where each slot handles a time
/// slice. As time progresses, the timer walks over the slot for the current
/// instant, and processes each entry for that slot. When the timer reaches the
/// end of the wheel, it starts again at the beginning.
///
/// The implementation maintains six wheels arranged in a set of levels. As the
/// levels go up, the slots of the associated wheel represent larger intervals
/// of time. At each level, the wheel has 64 slots. Each slot covers a range of
/// time equal to the wheel at the lower level. At level zero, each slot
/// represents one millisecond of time.
///
/// The wheels are:
///
/// * Level 0: 64 x 1 millisecond slots.
/// * Level 1: 64 x 64 millisecond slots.
/// * Level 2: 64 x ~4 second slots.
/// * Level 3: 64 x ~4 minute slots.
/// * Level 4: 64 x ~4 hour slots.
/// * Level 5: 64 x ~12 day slots.
///
/// When the timer processes entries at level zero, it will notify all the
/// `Sleep` instances as their deadlines have been reached. For all higher
/// levels, all entries will be redistributed across the wheel at the next level
/// down. Eventually, as time progresses, entries with [`Sleep`][sleep] instances will
/// either be canceled (dropped) or their associated entries will reach level
/// zero and be notified.
///
/// [paper]: http://www.cs.columbia.edu/~nahum/w6998/papers/ton97-timing-wheels.pdf
/// [sleep]: crate::time::Sleep
/// [timeout]: crate::time::Timeout
/// [interval]: crate::time::Interval
#[derive(Debug)]
pub(crate) struct TimeDriver {
    /// Parker to delegate to.
    ioStackEnum: IoStackEnum,
}

/// Wrapper around the sharded timer wheels.
struct ShardedWheel(Box<[Mutex<Wheel>]>);

impl TimeDriver {
    /// Creates a new `Driver` instance that uses `park` to block the current
    /// thread and `time_source` to get the current time and convert to ticks.
    ///
    /// Specifying the source of time is useful when testing.
    pub(crate) fn new(ioStackEnum: IoStackEnum, workerThreadCount: u32) -> (TimeDriver, TimeDriverHandle) {
        assert!(workerThreadCount > 0);

        let timeDriver = TimeDriver { ioStackEnum };

        let wheels: Vec<_> = (0..workerThreadCount).map(|_| Mutex::new(Wheel::new())).collect();

        let timeDriverHandle = TimeDriverHandle {
            timeSource: TimeSource::new(),
            inner: Inner {
                next_wake: AtomicOptionNonZeroU64::new(None),
                wheels: RwLock::new(ShardedWheel(wheels.into_boxed_slice())),
                wheels_len: workerThreadCount,
                is_shutdown: AtomicBool::new(false),
            },
        };


        (timeDriver, timeDriverHandle)
    }

    pub(crate) fn shutdown(&mut self, rt_handle: &driver::DriverHandle) {
        let handle = rt_handle.time();

        if handle.is_shutdown() {
            return;
        }

        handle.inner.is_shutdown.store(true, Ordering::SeqCst);

        // Advance time forward to the end of time.
        handle.process_at_time(0, u64::MAX);

        self.ioStackEnum.shutdown(rt_handle);
    }

    pub fn park_internal(&mut self, driverHandle: &DriverHandle, maxWaitDuration: Option<Duration>) {
        let timeDriverHandle = driverHandle.time();
        assert!(!timeDriverHandle.is_shutdown());

        // Finds out the min expiration time to park.
        let expiration_time = {
            let mut wheels = timeDriverHandle.inner.wheels.write().expect("Timer wheel shards poisoned");

            let expiration_time = wheels.0.iter_mut().filter_map(|wheel| wheel.get_mut().next_expiration_time()).min();

            timeDriverHandle.inner.next_wake.store(next_wake_time(expiration_time));

            expiration_time
        };

        match expiration_time {
            Some(when) => {
                let now = timeDriverHandle.timeSource.now();

                // Note that we effectively round up to 1ms here - this avoids
                // very short-duration microsecond-resolution sleeps that the OS
                // might treat as zero-length.
                let mut duration = timeDriverHandle.timeSource.tick_to_duration(when.saturating_sub(now));

                if duration > Duration::from_millis(0) {
                    if let Some(limit) = maxWaitDuration {
                        duration = std::cmp::min(limit, duration);
                    }

                    self.ioStackEnum.park_timeout(driverHandle, duration);
                } else {
                    self.ioStackEnum.park_timeout(driverHandle, Duration::from_secs(0));
                }
            }
            None => {
                if let Some(duration) = maxWaitDuration {
                    self.ioStackEnum.park_timeout(driverHandle, duration);
                } else {
                    self.ioStackEnum.park(driverHandle);
                }
            }
        }

        // Process pending timers after waking up
        timeDriverHandle.process();
    }
}

// Helper function to turn expiration_time into next_wake_time.
// Since the `park_timeout` will round up to 1ms for avoiding very
// short-duration microsecond-resolution sleeps, we do the same here.
// The conversion is as follows
// None => None
// Some(0) => Some(1)
// Some(i) => Some(i)
fn next_wake_time(expiration_time: Option<u64>) -> Option<NonZeroU64> {
    expiration_time.and_then(|v| {
        if v == 0 {
            NonZeroU64::new(1)
        } else {
            NonZeroU64::new(v)
        }
    })
}

impl TimeDriverHandle {
    /// Runs timer related logic, and returns the next wakeup time
    pub(self) fn process(&self) {
        let now = self.time_source().now();
        // For fairness, randomly select one to start.
        let shards = self.inner.get_shard_size();
        let start = crate::runtime::context::thread_rng_n(shards);
        self.process_at_time(start, now);
    }

    pub(self) fn process_at_time(&self, start: u32, now: u64) {
        let shards = self.inner.get_shard_size();

        let expiration_time = (start..shards + start).filter_map(|i| self.process_at_sharded_time(i, now)).min();

        self.inner.next_wake.store(next_wake_time(expiration_time));
    }

    // Returns the next wakeup time of this shard.
    pub(self) fn process_at_sharded_time(&self, id: u32, mut now: u64) -> Option<u64> {
        let mut waker_list = WakerList::new();
        let mut wheels_lock = self.inner.wheels.read().expect("Timer wheel shards poisoned");
        let mut lock = wheels_lock.lock_sharded_wheel(id);

        if now < lock.elapsed() {
            // Time went backwards! This normally shouldn't happen as the Rust language
            // guarantees that an Instant is monotonic, but can happen when running
            // Linux in a VM on a Windows host due to std incorrectly trusting the
            // hardware clock to be monotonic.
            //
            // See <https://github.com/tokio-rs/tokio/issues/3619> for more information.
            now = lock.elapsed();
        }

        while let Some(entry) = lock.poll(now) {
            debug_assert!(unsafe { entry.is_pending() });

            // SAFETY: We hold the driver lock, and just removed the entry from any linked lists.
            if let Some(waker) = unsafe { entry.fire(Ok(())) } {
                waker_list.push(waker);

                if !waker_list.can_push() {
                    // Wake a batch of wakers. To avoid deadlock, we must do this with the lock temporarily dropped.
                    drop(lock);
                    drop(wheels_lock);

                    waker_list.wake_all();

                    wheels_lock = self.inner.wheels.read().expect("Timer wheel shards poisoned");
                    lock = wheels_lock.lock_sharded_wheel(id);
                }
            }
        }
        let next_wake_up = lock.poll_at();
        drop(lock);
        drop(wheels_lock);

        waker_list.wake_all();
        next_wake_up
    }

    /// Removes a registered timer from the driver.
    ///
    /// The timer will be moved to the cancelled state. Wakers will _not_ be
    /// invoked. If the timer is already completed, this function is a no-op.
    ///
    /// This function always acquires the driver lock, even if the entry does
    /// not appear to be registered.
    ///
    /// SAFETY: The timer must not be registered with some other driver, and
    /// `add_entry` must not be called concurrently.
    pub(self) unsafe fn clear_entry(&self, entry: NonNull<TimerShared>) {
        unsafe {
            let wheels_lock = self.inner.wheels.read().expect("Timer wheel shards poisoned");
            let mut lock = wheels_lock.lock_sharded_wheel(entry.as_ref().shard_id());

            if entry.as_ref().might_be_registered() {
                lock.remove(entry);
            }

            entry.as_ref().handle().fire(Ok(()));
        }
    }

    /// Removes and re-adds an entry to the driver.
    ///
    /// SAFETY: The timer must be either unregistered, or registered with this
    /// driver. No other threads are allowed to concurrently manipulate the
    /// timer at all (the current thread should hold an exclusive reference to
    /// the `TimerEntry`)
    pub(self) unsafe fn reRegister(&self,
                                   unpark: &IOHandleEnum,
                                   new_tick: u64,
                                   entry: NonNull<TimerShared>) {
        let waker = unsafe {
            let wheels_lock = self.inner.wheels.read().expect("Timer wheel shards poisoned");

            let mut lock = wheels_lock.lock_sharded_wheel(entry.as_ref().shard_id());

            // We may have raced with a firing/deregistration, so check before
            // deregistering.
            if unsafe { entry.as_ref().might_be_registered() } {
                lock.remove(entry);
            }

            // Now that we have exclusive control of this entry, mint a handle to reinsert it.
            let entry = entry.as_ref().handle();

            if self.is_shutdown() {
                unsafe { entry.fire(Err(Error::shutdown())) }
            } else {
                entry.set_expiration(new_tick);

                // Note: We don't have to worry about racing with some other resetting
                // thread, because add_entry and reregister require exclusive control of
                // the timer entry.
                match unsafe { lock.insert(entry) } {
                    Ok(when) => {
                        if self.inner.next_wake.load().map(|next_wake| when < next_wake.get()).unwrap_or(true) {
                            unpark.unpark();
                        }

                        None
                    }
                    Err((entry, crate::time::error::InsertError::Elapsed)) => unsafe {
                        entry.fire(Ok(()))
                    },
                }
            }

            // Must release lock before invoking waker to avoid the risk of deadlock.
        };

        // The timer was fired synchronously as a result of the reregistration.
        // Wake the waker; this is needed because we might reset _after_ a poll,
        // and otherwise the task won't be awoken to poll again.
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Timer state shared between `Driver`, `Handle`, and `Registration`.
struct Inner {
    /// The earliest time at which we promise to wake up without unparking.
    next_wake: AtomicOptionNonZeroU64,

    /// Sharded Timer wheels.
    wheels: RwLock<ShardedWheel>,

    /// Number of entries in the sharded timer wheels.
    wheels_len: u32,

    /// True if the driver is being shutdown.
    pub(super) is_shutdown: AtomicBool,
}


impl Inner {
    // Check whether the driver has been shutdown
    pub(super) fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    // Gets the number of shards.
    fn get_shard_size(&self) -> u32 {
        self.wheels_len
    }
}

impl fmt::Debug for Inner {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Inner").finish()
    }
}

impl ShardedWheel {
    /// Locks the driver's sharded wheel structure.
    pub(super) fn lock_sharded_wheel(&self, shard_id: u32) -> crate::loom::sync::MutexGuard<'_, Wheel> {
        let index = shard_id % (self.0.len() as u32);
        // Safety: This modulo operation ensures that the index is not out of bounds.
        unsafe { self.0.get_unchecked(index as usize) }.lock()
    }
}
