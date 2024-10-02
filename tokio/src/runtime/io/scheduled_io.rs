use crate::io::interest::Interest;
use crate::io::ready::Ready;
use crate::loom::sync::atomic::AtomicUsize;
use crate::loom::sync::Mutex;
use crate::runtime::io::{Direction, ReadyEvent, Tick};
use crate::util::bit;
use crate::util::linked_list::{self, LinkedList};
use crate::util::WakerList;

use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{AcqRel, Acquire};
use std::task::{Context, Poll, Waker};

/// Stored in the I/O driver resource slab.
#[derive(Debug)]
// # This struct should be cache padded to avoid false sharing. The cache padding rules are copied
// from crossbeam-utils/src/cache_padded.rs
//
// Starting from Intel's Sandy Bridge, spatial prefetcher is now pulling pairs of 64-byte cache
// lines at a time, so we have to align to 128 bytes rather than 64.
//
// Sources:
// - https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-optimization-manual.pdf
// - https://github.com/facebook/folly/blob/1b5288e6eea6df074758f877c849b6e73bbb9fbb/folly/lang/Align.h#L107
//
// ARM's big.LITTLE architecture has asymmetric cores and "big" cores have 128-byte cache line size.
//
// Sources:
// - https://www.mono-project.com/news/2016/09/12/arm64-icache/
//
// powerpc64 has 128-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_ppc64x.go#L9
#[cfg_attr(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
    ),
    repr(align(128))
)]
// arm, mips, mips64, sparc, and hexagon have 32-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_arm.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mips.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mipsle.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mips64x.go#L9
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/sparc/include/asm/cache.h#L17
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/hexagon/include/asm/cache.h#L12
#[cfg_attr(
    any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "sparc",
        target_arch = "hexagon",
    ),
    repr(align(32))
)]
// m68k has 16-byte cache line size.
//
// Sources:
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/m68k/include/asm/cache.h#L9
#[cfg_attr(target_arch = "m68k", repr(align(16)))]
// s390x has 256-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_s390x.go#L7
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/s390/include/asm/cache.h#L13
#[cfg_attr(target_arch = "s390x", repr(align(256)))]
// x86, riscv, wasm, and sparc64 have 64-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/dda2991c2ea0c5914714469c4defc2562a907230/src/internal/cpu/cpu_x86.go#L9
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_wasm.go#L7
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/sparc/include/asm/cache.h#L19
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/riscv/include/asm/cache.h#L10
//
// All others are assumed to have 64-byte cache line size.
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "sparc",
        target_arch = "hexagon",
        target_arch = "m68k",
        target_arch = "s390x",
    )),
    repr(align(64))
)]
pub(crate) struct ScheduledIO {
    pub(super) linked_list_pointers: UnsafeCell<linked_list::Pointers<ScheduledIO>>,

    /// Packs the resource's readiness and I/O driver latest tick.
    readyValue: AtomicUsize,

    waiters: Mutex<Waiters>,
}

#[derive(Debug, Default)]
struct Waiters {
    /// List of all current waiters.
    waiterList: LinkedList<Waiter, <Waiter as linked_list::Link>::Target>,

    /// Waker used for `AsyncRead`.
    readWaker: Option<Waker>,

    /// Waker used for `AsyncWrite`.
    writeWaker: Option<Waker>,
}

#[derive(Debug)]
struct Waiter {
    pointers: linked_list::Pointers<Waiter>,

    /// The waker for this task.
    waker: Option<Waker>,

    /// The interest this waiter is waiting on.
    interest: Interest,

    isReady: bool,

    /// Should never be `!Unpin`.
    _p: PhantomPinned,
}

impl Waiter {
    unsafe fn addr_of_pointers(me: NonNull<Self>) -> NonNull<linked_list::Pointers<Waiter>> {
        let me = me.as_ptr();
        let field = &raw mut (*me).pointers;
        NonNull::new_unchecked(field)
    }
}

unsafe impl linked_list::Link for Waiter {
    type Handle = NonNull<Waiter>;
    type Target = Waiter;

    fn as_raw(handle: &NonNull<Waiter>) -> NonNull<Waiter> {
        *handle
    }

    unsafe fn from_raw(ptr: NonNull<Waiter>) -> NonNull<Waiter> {
        ptr
    }

    unsafe fn pointers(target: NonNull<Waiter>) -> NonNull<linked_list::Pointers<Waiter>> {
        Waiter::addr_of_pointers(target)
    }
}

struct ReadyFuture<'a> {
    scheduledIO: &'a ScheduledIO,
    state: ReadyFutureState,
    /// Entry in the waiter `LinkedList`.
    waiter: UnsafeCell<Waiter>,
}

enum ReadyFutureState {
    Init,
    Waiting,
    Done,
}

// Ready值的构成如下
//
// | shutdown | driver tick | readiness 真正的和epoll对应的ready信息 |
// |----------+-------------+-------------------------------------|
// |   1 bit  |  15 bits    +   16 bits                           |

const READINESS: bit::Pack = bit::Pack::least_significant(16);
const TICK: bit::Pack = READINESS.then(15);
const SHUTDOWN: bit::Pack = TICK.then(1);

impl ScheduledIO {
    pub(crate) fn token(&self) -> mio::Token {
        // use `expose_addr` when stable
        mio::Token(self as *const _ as usize)
    }

    /// Invoked when the IO driver is shut down; forces this `ScheduledIo` into a  permanently shutdown state.
    pub(super) fn shutdown(&self) {
        let mask = SHUTDOWN.pack(1, 0);
        self.readyValue.fetch_or(mask, AcqRel);
        self.wake(Ready::ALL);
    }

    /// Sets the readiness on this `ScheduledIo` by invoking the given closure on
    /// the current value, returning the previous readiness value.
    ///
    /// # Arguments
    /// - `tick`: whether setting the tick or trying to clear readiness for a specific tick
    /// - `f`: a closure returning a new readiness value given the previous readiness
    pub(super) fn setReadyValue(&self, tick: Tick, f: impl Fn(Ready) -> Ready) {
        let mut currentReadyVal = self.readyValue.load(Acquire);

        // If the io driver is shut down, then you are only allowed to clear readiness.
        debug_assert!(SHUTDOWN.unpack(currentReadyVal) == 0 || matches!(tick, Tick::Clear(_)));

        loop {
            // Mask out the tick bits so that the modifying function doesn't see them.
            let currentReady = Ready::from_usize(currentReadyVal);
            let newReady = f(currentReady);

            let new_tick = match tick {
                Tick::Set => {
                    let currentTick = TICK.unpack(currentReadyVal);
                    currentTick.wrapping_add(1) % (TICK.max_value() + 1)
                }
                Tick::Clear(t) => {
                    // Trying to clear readiness with an old event
                    if TICK.unpack(currentReadyVal) as u8 != t {
                        return;
                    }

                    t as usize
                }
            };

            let next = TICK.pack(new_tick, newReady.as_usize());

            match self.readyValue.compare_exchange(currentReadyVal, next, AcqRel, Acquire) {
                Ok(_) => return,
                Err(actual) => currentReadyVal = actual,  // lost the race, retry
            }
        }
    }

    /// Notifies all pending waiters that have registered interest in `ready`.
    ///
    /// There may be many waiters to notify. Waking the pending task **must** be
    /// done from outside of the lock otherwise there is a potential for a
    /// deadlock.
    ///
    /// A stack array of wakers is created and filled with wakers to notify, the
    /// lock is released, and the wakers are notified. Because there may be more
    /// than 32 wakers to notify, if the stack array fills up, the lock is
    /// released, the array is cleared, and the iteration continues.
    pub(super) fn wake(&self, ready: Ready) {
        let mut wakerList = WakerList::new();

        let mut waiters = self.waiters.lock();

        // check for AsyncRead slot
        if ready.is_readable() {
            if let Some(waker) = waiters.readWaker.take() {
                wakerList.push(waker);
            }
        }

        // check for AsyncWrite slot
        if ready.is_writable() {
            if let Some(waker) = waiters.writeWaker.take() {
                wakerList.push(waker);
            }
        }

        'outer:
        loop {
            let mut iter = waiters.waiterList.drain_filter(|waiter| ready.satisfies(waiter.interest));

            while wakerList.can_push() {
                match iter.next() {
                    Some(waiter) => {
                        let waiter = unsafe { &mut *waiter.as_ptr() };

                        // waiter上已有的waker也添加到list
                        if let Some(waker) = waiter.waker.take() {
                            waiter.isReady = true;
                            wakerList.push(waker);
                        }
                    }
                    None => break 'outer
                }
            }

            // release the lock before notifying
            drop(waiters);
            wakerList.wake_all();

            // acquire the lock again
            waiters = self.waiters.lock();
        }

        drop(waiters);
        wakerList.wake_all();
    }

    pub(super) fn ready_event(&self, interest: Interest) -> ReadyEvent {
        let curr = self.readyValue.load(Acquire);

        ReadyEvent {
            tick: TICK.unpack(curr) as u8,
            ready: interest.mask() & Ready::from_usize(READINESS.unpack(curr)),
            isShutdown: SHUTDOWN.unpack(curr) != 0,
        }
    }

    /// Polls for readiness events in a given direction.
    ///
    /// These are to support `AsyncRead` and `AsyncWrite` polling methods,
    /// which cannot use the `async fn` version. This uses reserved reader and writer slots.
    pub(super) fn pollReady(&self, cx: &mut Context<'_>, direction: Direction) -> Poll<ReadyEvent> {
        let currentReadyValue = self.readyValue.load(Acquire);

        let ready = direction.mask() & Ready::from_usize(READINESS.unpack(currentReadyValue));
        let is_shutdown = SHUTDOWN.unpack(currentReadyValue) != 0;

        // 当未满足要求的时候set相应的waker
        if ready.is_empty() && !is_shutdown {
            let mut waiters = self.waiters.lock();
            let slot = match direction {
                Direction::Read => &mut waiters.readWaker,
                Direction::Write => &mut waiters.writeWaker,
            };

            // avoid cloning the waker if one is already stored that matches the current task.
            match slot {
                Some(existing) => {
                    if !existing.will_wake(cx.waker()) {
                        existing.clone_from(cx.waker());
                    }
                }
                None => *slot = Some(cx.waker().clone()),
            }

            // try again, in case the readiness was changed while we were taking the waiters lock
            let curr = self.readyValue.load(Acquire);
            let ready = direction.mask() & Ready::from_usize(READINESS.unpack(curr));
            let is_shutdown = SHUTDOWN.unpack(curr) != 0;
            if is_shutdown {
                Poll::Ready(ReadyEvent {
                    tick: TICK.unpack(curr) as u8,
                    ready: direction.mask(),
                    isShutdown: is_shutdown,
                })
            } else if ready.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(ReadyEvent {
                    tick: TICK.unpack(curr) as u8,
                    ready,
                    isShutdown: is_shutdown,
                })
            }
        } else {
            Poll::Ready(ReadyEvent {
                tick: TICK.unpack(currentReadyValue) as u8,
                ready,
                isShutdown: is_shutdown,
            })
        }
    }

    pub(crate) fn clear_readiness(&self, event: ReadyEvent) {
        // This consumes the current readiness state **except** for closed
        // states. Closed states are excluded because they are final states.
        let mask_no_closed = event.ready - Ready::READ_CLOSED - Ready::WRITE_CLOSED;
        self.setReadyValue(Tick::Clear(event.tick), |curr| curr - mask_no_closed);
    }

    pub(crate) fn clear_wakers(&self) {
        let mut waiters = self.waiters.lock();
        waiters.readWaker.take();
        waiters.writeWaker.take();
    }

    /// An async version of `poll_readiness` which uses a linked list of wakers.
    pub(crate) async fn pollReadinessAsync(&self, interest: Interest) -> ReadyEvent {
        ReadyFuture {
            scheduledIO: self,
            state: ReadyFutureState::Init,
            waiter: UnsafeCell::new(Waiter {
                pointers: linked_list::Pointers::new(),
                waker: None,
                isReady: false,
                interest,
                _p: PhantomPinned,
            }),
        }.await
    }
}

unsafe impl Send for ScheduledIO {}
unsafe impl Sync for ScheduledIO {}

impl Drop for ScheduledIO {
    fn drop(&mut self) {
        self.wake(Ready::ALL);
    }
}

impl Default for ScheduledIO {
    fn default() -> ScheduledIO {
        ScheduledIO {
            linked_list_pointers: UnsafeCell::new(linked_list::Pointers::new()),
            readyValue: AtomicUsize::new(0),
            waiters: Mutex::new(Waiters::default()),
        }
    }
}

impl Future for ReadyFuture<'_> {
    type Output = ReadyEvent;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        use std::sync::atomic::Ordering::SeqCst;

        let (scheduled_io, state, waiter) = unsafe {
            let me = self.get_unchecked_mut();
            (&me.scheduledIO, &mut me.state, &me.waiter)
        };

        loop {
            match *state {
                ReadyFutureState::Init => {
                    // Optimistically check existing readiness
                    let curr = scheduled_io.readyValue.load(SeqCst);
                    let ready = Ready::from_usize(READINESS.unpack(curr));
                    let is_shutdown = SHUTDOWN.unpack(curr) != 0;
                    // `waiter.interest` never changes
                    let interest = unsafe { (*waiter.get()).interest };
                    let ready = ready.intersection(interest);
                    if !ready.is_empty() || is_shutdown {
                        // ready
                        let tick = TICK.unpack(curr) as u8;
                        *state = ReadyFutureState::Done;
                        return Poll::Ready(ReadyEvent {
                            tick,
                            ready,
                            isShutdown: is_shutdown,
                        });
                    }

                    // not ready, take the lock (and check again while locked).
                    let mut waiters = scheduled_io.waiters.lock();

                    let curr = scheduled_io.readyValue.load(SeqCst);
                    let mut ready = Ready::from_usize(READINESS.unpack(curr));
                    let is_shutdown = SHUTDOWN.unpack(curr) != 0;
                    if is_shutdown {
                        ready = Ready::ALL;
                    }
                    let ready = ready.intersection(interest);
                    if !ready.is_empty() || is_shutdown {
                        // currently ready!
                        let tick = TICK.unpack(curr) as u8;
                        *state = ReadyFutureState::Done;
                        return Poll::Ready(ReadyEvent {
                            tick,
                            ready,
                            isShutdown: is_shutdown,
                        });
                    }

                    // Not ready even after locked, insert into list...

                    // Safety: called while locked
                    unsafe { (*waiter.get()).waker = Some(context.waker().clone()); }

                    // safety: pointers from `UnsafeCell` are never null.
                    waiters.waiterList.push_front(unsafe { NonNull::new_unchecked(waiter.get()) });

                    *state = ReadyFutureState::Waiting;
                }
                ReadyFutureState::Waiting => {
                    let waiters = scheduled_io.waiters.lock();

                    let w = unsafe { &mut *waiter.get() };

                    // scheduleInfo.wake(ready) 会更改isReady
                    if w.isReady {  // waker has been notified.
                        *state = ReadyFutureState::Done;
                    } else { // update the waker, if necessary.
                        if !w.waker.as_ref().unwrap().will_wake(context.waker()) {
                            w.waker = Some(context.waker().clone());
                        }

                        return Poll::Pending;
                    }

                    drop(waiters);
                }
                ReadyFutureState::Done => {
                    // Safety: State::Done means it is no longer shared
                    let w = unsafe { &mut *waiter.get() };

                    let curr = scheduled_io.readyValue.load(Acquire);
                    let is_shutdown = SHUTDOWN.unpack(curr) != 0;

                    // The returned tick might be newer than the event
                    // which notified our waker. This is ok because the future
                    // still didn't return `Poll::Ready`.
                    let tick = TICK.unpack(curr) as u8;

                    // The readiness state could have been cleared in the meantime,
                    // but we allow the returned ready set to be empty.
                    let curr_ready = Ready::from_usize(READINESS.unpack(curr));
                    let ready = curr_ready.intersection(w.interest);

                    return Poll::Ready(ReadyEvent {
                        tick,
                        ready,
                        isShutdown: is_shutdown,
                    });
                }
            }
        }
    }
}

impl Drop for ReadyFuture<'_> {
    fn drop(&mut self) {
        let mut waiters = self.scheduledIO.waiters.lock();

        // Safety: `waiter` is only ever stored in `waiters`
        unsafe {
            waiters.waiterList.remove(NonNull::new_unchecked(self.waiter.get()))
        };
    }
}

unsafe impl Send for ReadyFuture<'_> {}
unsafe impl Sync for ReadyFuture<'_> {}
