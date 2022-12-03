use std::borrow::Borrow;
use std::cell::UnsafeCell;
use std::fmt::{Debug, Display, Formatter};
use std::intrinsics::unlikely;
use std::marker::PhantomData;
use std::mem::{align_of, ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{mem, ptr, thread};
use thread_local::ThreadLocal;

/*
#[derive(Copy, Clone, Debug)]
pub enum UpdateResult {
    Ok,
    AlreadyUpdating,
    NoUpdate,
}

// const IDLE_MARKER: usize = 1 << 0;

const fn most_sig_set_bit(val: usize) -> Option<u32> {
    let mut i = 0;
    let mut ret = None;
    while i < usize::BITS {
        if val & (1 << i) != 0 {
            ret = Some(i);
        }
        i += 1;
    }
    ret
}

const fn assert_alignment<T, const METADATA_BITS: u32>() -> bool {
    let free_bits = most_sig_set_bit(align_of::<T>()).unwrap_or(0);
    if free_bits < METADATA_BITS + 1 {
        unreachable!("The alignment of T is insufficient, expected `{}`, but found `{}`", 1 << (METADATA_BITS + 1), 1 << free_bits);
    }
    true
}

const GLOBAL_UPDATE_FLAG: usize = 1 << 0;

// FIXME: note somewhere that this data structure requires T to at least be aligned to 2 bytes
// FIXME: and that the least significant bit of T's pointer is used internally, also add a static assertion for this!
/// A `SwapArc` is a data structure that allows for an `Arc`
/// to be passed around and swapped out with other `Arc`s.
/// In order to achieve this, an internal reference count
/// scheme is used which allows for very quick, low overhead
/// reads in the common case (no update) and will sill be
/// decently fast when an update is performed, as updates
/// only consist of 3 atomic instructions. When a new
/// `Arc` is to be stored in the `SwapArc`, it first tries to
/// immediately update the current pointer (the one all readers will see)
/// (this is possible, if no other update is being performed and if there are no readers left)
/// if this fails, it will `push` the update so that it will
/// be performed by the last reader to finish reading.
/// A read consists of loading the current pointer and
/// performing a clone operation on the `Arc`, thus
/// readers are very short-lived and shouldn't block
/// updates for very long, although writer starvation
/// is possible in theory, it probably won't every be
/// observed in practice because of the short-lived
/// nature of readers.

/// This variant of `SwapArc` has wait-free reads (although
/// this is at the cost of additional atomic instructions
/// (at least 1 additional load - this will never be more than 1 load if there are no updates happening).
pub struct SwapArcIntermediateTLS<T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_HEADER_BITS: u32 = 0> {
    // FIXME: support metadata - how can we do that?
    // FIXME: we could maybe do this by putting the metadata inside of the curr and intermediate atomic ptrs
    // FIXME: inside the SwapArc itself - the major issue we have is that with this approach we loose basically all the benefits
    // FIXME: of using thread locals as we have to maintain the same structure inside the SwapArc as with the old `SwapArc` (without tls)
    // FIXME: this is because we have to maintain the same old ref counter using atomic fetch_add and fetch_sub instructions
    // FIXME: this prevents new updates from happening (probably)
    // FIXME: IMPORTANT: if we want to solve the metadata issue if we have to solve the compare_exchange issue first
    // FIXME: because the metadata solution has to take the compare_exchange behavior into account
    updated: AtomicPtr<T>,
    curr: AtomicPtr<T>, // 1 bit: updating | 63 bits: ptr
    thread_local: ThreadLocal<LocalData<T, D, METADATA_HEADER_BITS>>,
    updating: Mutex<bool>,
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> SwapArcIntermediateTLS<T, D, METADATA_PREFIX_BITS> {

    pub fn new(val: D) -> Arc<Self> {
        static_assertions::const_assert!(assert_alignment::<T, { METADATA_PREFIX_BITS }>());
        let val = ManuallyDrop::new(val);
        let virtual_ref = val.as_ptr();
        Arc::new(Self {
            updated: AtomicPtr::new(null_mut()),
            curr: AtomicPtr::new(virtual_ref.cast_mut()),
            thread_local: ThreadLocal::new(),
            updating: Mutex::new(false),
        })
    }

    /// SAFETY: this is only safe to call if the caller increments the
    /// reference count of the "object" `val` points to.
    fn dummy0() {}
    /*unsafe fn new_raw(val: *const T) -> Arc<Self> {
        Arc::new(Self {
            curr_ref_cnt: Default::default(),
            ptr: AtomicPtr::new(val.cast_mut()),
            intermediate_ref_cnt: Default::default(),
            intermediate_ptr: AtomicPtr::new(null_mut()),
            updated: AtomicPtr::new(null_mut()),
            thread_local: ThreadLocal::new(),
            _phantom_data: Default::default(),
        })
    }*/

    pub fn load<'a>(self: &'a Arc<Self>) -> SwapArcIntermediateGuard<'a, T, D, METADATA_PREFIX_BITS> {
        let mut new = false;
        let parent = self.thread_local.get_or(|| {
            new = true;
            /*LocalData {
                inner: MaybeUninit::new(self.clone().load_internal()),
                ref_cnt: 1,
            }*/
            let mut curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst); // FIXME: is it okay to have a blocking solution here?
            let mut back_off = 1;
            while curr & GLOBAL_UPDATE_FLAG != 0 {
                curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst);
                // back-off
                thread::sleep(Duration::from_micros(10 * back_off));
                back_off += 1;
            }
            // increase the reference count
            let tmp = ManuallyDrop::new(D::from(curr));
            mem::forget(tmp.clone());
            // we know the current state of the ptr stored in curr
            self.curr.fetch_and(GLOBAL_UPDATE_FLAG, Ordering::Release);
            LocalData {
                parent: self.clone(),
                intermediate_update: AtomicPtr::new(curr),
                update: AtomicPtr::new(curr),
                inner: UnsafeCell::new(LocalDataInner {
                    intermediate_ptr: null(),
                    intermediate: LocalCounted { val: MaybeUninit::uninit(), ref_cnt: 0, _phantom_data: Default::default() },
                    new_ptr: null(),
                    new_src: RefSource::Curr,
                    new: LocalCounted { val: MaybeUninit::uninit(), ref_cnt: 0, _phantom_data: Default::default() },
                    // FIXME: increase ref count
                    curr: LocalCounted { val: MaybeUninit::new(D::from(curr)), ref_cnt: 1, _phantom_data: Default::default() }
                }),
            }
        });
        // SAFETY: This is safe because we know that we are the only thread that
        // is able to access the thread local data at this time and said data has to be initialized
        // and we also know, that the pointer has to be non-null
        let data = unsafe { parent.inner.get().as_mut().unwrap_unchecked() };
        if unlikely(new) {
            let fake_ref = ManuallyDrop::new(D::from(data.val.as_ptr()));
            return SwapArcIntermediateGuard {
                parent,
                fake_ref,
            };
        }
        /*
        data.ref_cnt += 1;
        if data.intermediate_ptr == data.new_ptr {
            if parent.update.compare_exchange(!STATE_IN_USE, Ordering::SeqCst).is_ok() {

            }
            let loaded =  // FIXME: try reducing this ordering!
                .map_addr(|x| x & !IDLE_MARKER);
            if !loaded.is_null() {
                data.new_val_ptr = loaded;
                data.new_val = ManuallyDrop::new(D::from(loaded));
            }
        } else {

        }
        // FIXME: try using intermediate value instead
        let fake_ref = ManuallyDrop::new(D::from(data.val.as_ptr()));
        SwapArcIntermediateGuard {
            parent,
            // SAFETY: we know that this is safe because the ref count is non-zero
            fake_ref,
        }*/
        if data.new_ptr.is_null() {
            let ptr = if parent.update.load(Ordering::Acquire) != unsafe { data.curr.val.assume_init_ref() }.as_ptr() {
                let loaded = parent.load_update();
                data.new_ptr = loaded.0;
                data.new_src = loaded.1;
                data.new = LocalCounted {
                    val: MaybeUninit::new(D::from(loaded.0)),
                    ref_cnt: 1,
                    _phantom_data: Default::default()
                };
                loaded.0.cast_const()
            } else {
                data.curr.ref_cnt += 1;
                unsafe { data.curr.val.assume_init_ref() }.as_ptr()
            };
            // FIXME: perform an update! - this is probably fixed
            let fake_ref = ManuallyDrop::new(D::from(ptr));
            return SwapArcIntermediateGuard {
                parent,
                // SAFETY: we know that this is safe because the ref count is non-zero
                fake_ref,
            };
        }
        if data.intermediate_ptr.is_null() {
            let intermediate = parent.intermediate_update.load(Ordering::Acquire);
            // check if there is a new intermediate value and that the intermediate value has been verified to be usable
            // (it doesn't have the `IN_USE_FLAG` and the `UPDATE_FLAG` set)
            let ptr = if intermediate.expose_addr() & META_DATA_MASK != META_DATA_MASK &&
                intermediate.cast_const() != data.new_ptr {
                let loaded = parent.intermediate_update.fetch_or(IN_USE_FLAG, Ordering::SeqCst).map_addr(|x| x & !META_DATA_MASK);
                data.intermediate_ptr = loaded.0;
                data.intermediate = LocalCounted {
                    val: MaybeUninit::new(D::from(loaded)),
                    ref_cnt: 1,
                    _phantom_data: Default::default()
                };
                loaded.cast_const()
            } else {
                data.new.ref_cnt += 1;
                data.new_ptr
            };
            // FIXME: perform an update! - this is probably fixed
            let fake_ref = ManuallyDrop::new(D::from(ptr));
            return SwapArcIntermediateGuard {
                parent,
                // SAFETY: we know that this is safe because the ref count is non-zero
                fake_ref,
            };
        } else {
            let fake_ref = ManuallyDrop::new(D::from(data.intermediate_ptr));
            return SwapArcIntermediateGuard {
                parent: &parent,
                fake_ref,
            };
        }
    }

    pub fn load_full(self: &Arc<Self>) -> D {
        self.load().as_ref().clone()
    }

    pub unsafe fn load_raw<'a>(self: &'a Arc<Self>) -> SwapArcIntermediatePtrGuard<'a, T, D, METADATA_PREFIX_BITS> {
        let guard = ManuallyDrop::new(self.load());
        SwapArcIntermediatePtrGuard {
            parent: guard.parent,
            ptr: guard.fake_ref.as_ptr(),
        }
    }

    /*
    fn try_update_curr(&self) -> bool {
        match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                // FIXME: can we somehow bypass intermediate if we have a new update upcoming - we probably can't because this would probably cause memory leaks and other funny things that we don't like
                let intermediate = self.intermediate_ptr.load(Ordering::SeqCst);
                // update the pointer
                let prev = self.ptr.load(Ordering::Acquire);
                if Self::strip_metadata(prev) != Self::strip_metadata(intermediate) {
                    self.ptr.store(intermediate, Ordering::Release);
                    // unset the update flag
                    self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                    println!("UPDATE status: {}", (self.intermediate_ref_cnt.load(Ordering::SeqCst) & Self::UPDATE != 0));
                    // unset the `weak` update flag from the intermediate ref cnt
                    self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst); // FIXME: are we sure this can't happen if there is UPDATE set for intermediate_ref?
                    // drop the `virtual reference` we hold to the Arc
                    D::from(Self::strip_metadata(prev));
                } else {
                    // unset the update flag
                    self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                }
                true
            }
            _ => false,
        }
    }

    fn try_update_intermediate(&self) {
        match self.intermediate_ref_cnt.compare_exchange(0, Self::UPDATE | Self::OTHER_UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                // take the update
                let update = self.updated.swap(null_mut(), Ordering::SeqCst);
                // check if we even have an update
                if !update.is_null() {
                    let metadata = Self::get_metadata(self.intermediate_ptr.load(Ordering::Acquire));
                    let update = Self::merge_ptr_and_metadata(update, metadata).cast_mut();
                    self.intermediate_ptr.store(update, Ordering::Release);
                    // unset the update flag
                    self.intermediate_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                    // try finishing the update up!
                    match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => {
                            let prev = self.ptr.swap(update, Ordering::Release);
                            // unset the update flag
                            self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                            // unset the `weak` update flag from the intermediate ref cnt
                            self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst);
                            // drop the `virtual reference` we hold to the Arc
                            D::from(Self::strip_metadata(prev));
                        }
                        Err(_) => {}
                    }
                } else {
                    // unset the update flags
                    self.intermediate_ref_cnt.fetch_and(!(Self::UPDATE | Self::OTHER_UPDATE), Ordering::SeqCst);
                }
            }
            Err(_) => {}
        }
    }*/

    fn try_update(&self, val: *const T) -> Option<bool> {
        let curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst);
        if curr & GLOBAL_UPDATE_FLAG != 0 {
            return None;
        }
        for local in self.thread_local.iter() {
            let mapped = val.map_addr(|x| x | UPDATE_FLAG | IN_USE_FLAG).cast_mut();
            // the thread this `local` belongs to didn't already update and
            // it isn't idling
            // note: the `UPDATE_FLAG` has a different meaning for `intermediate` and for `update` (although one could say that it has the exact sane meaning)
            // for `intermediate` it means that `update` has a pending update, but if `IN_USE_FLAG` is set for it as well, it means it is currently being updated
            // for `update` it means that itself has a pending update
            if !local.intermediate_update.compare_exchange(curr, mapped, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                // fixup states for all thread locals
                for local in self.thread_local.iter() {
                    if !local.intermediate_update.compare_exchange(mapped, local.update.load(Ordering::SeqCst).map_addr(|x| x & !META_DATA_MASK), Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                        break;
                    }
                }
                return Some(false);
            }
        }
        for local in self.thread_local.iter() {
            local.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
            let curr = local.update.fetch_or(UPDATE_FLAG, Ordering::SeqCst);
            if curr.expose_addr() & IN_USE_FLAG == 0 {
                local.update.store(val.cast_mut(), Ordering::SeqCst);
                local.intermediate_update.fetch_and(!UPDATE_FLAG, Ordering::SeqCst);
            }
        }
        self.curr.store(val.cast_mut(), Ordering::Release);
        Some(true)
    }

    fn try_update_locals(&self, curr: *mut T, val: *const T) -> bool {
        for local in self.thread_local.iter() {
            let mapped = val.map_addr(|x| x | UPDATE_FLAG | IN_USE_FLAG).cast_mut();
            // the thread this `local` belongs to didn't already update and
            // it isn't idling
            // note: the `UPDATE_FLAG` has a different meaning for `intermediate` and for `update` (although one could say that it has the exact sane meaning)
            // for `intermediate` it means that `update` has a pending update, but if `IN_USE_FLAG` is set for it as well, it means it is currently being updated
            // for `update` it means that itself has a pending update
            if !local.intermediate_update.compare_exchange(curr, mapped, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                // fixup states for all thread locals
                for local in self.thread_local.iter() {
                    if !local.intermediate_update.compare_exchange(mapped, local.update.load(Ordering::SeqCst).map_addr(|x| x & !META_DATA_MASK), Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                        break;
                    }
                }
                return false;
            }
        }
        for local in self.thread_local.iter() {
            local.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
            let curr = local.update.fetch_or(UPDATE_FLAG, Ordering::SeqCst);
            if curr.expose_addr() & IN_USE_FLAG == 0 {
                local.update.store(val.cast_mut(), Ordering::Release); // FIXME: this can probably lead to a data race with thread locals loading things (and setting the IN_USE_FLAG in the process)
                local.intermediate_update.fetch_and(!UPDATE_FLAG, Ordering::Release);
            }
        }
        true
    }

    pub fn update(&self, updated: D) {
        updated.increase_ref_cnt();
        unsafe { self.update_raw(updated.into()); }
    }

    unsafe fn update_raw(&self, updated: *const T) {
        /*let updated = Self::strip_metadata(updated);
        loop {
            match self.intermediate_ref_cnt.compare_exchange(0, Self::UPDATE | Self::OTHER_UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    let new = updated.cast_mut();
                    let tmp = ManuallyDrop::new(D::from(new));
                    tmp.increase_ref_cnt();
                    // clear out old updates to make sure our update won't be overwritten by them in the future
                    let old = self.updated.swap(null_mut(), Ordering::SeqCst);
                    let metadata = Self::get_metadata(self.intermediate_ptr.load(Ordering::Acquire));
                    let new = Self::merge_ptr_and_metadata(new, metadata).cast_mut();
                    self.intermediate_ptr.store(new, Ordering::Release);
                    // unset the update flag
                    self.intermediate_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                    if !old.is_null() {
                        // drop the `virtual reference` we hold to the Arc
                        D::from(old);
                    }
                    // try finishing the update up!
                    match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => {
                            let prev = self.ptr.swap(new, Ordering::Release);
                            // unset the update flag
                            self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                            // unset the `weak` update flag from the intermediate ref cnt
                            self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst);
                            // drop the `virtual reference` we hold to the Arc
                            D::from(Self::strip_metadata(prev));
                        }
                        Err(_) => {}
                    }
                    break;
                }
                Err(old) => {
                    if old & Self::UPDATE != 0 { // FIXME: what about Self::UPDATE_OTHER?
                        // somebody else already updates the current ptr, so we wait until they finish their update
                        continue;
                    }
                    // push our update up, so it will be applied in the future
                    let old = self.updated.swap(updated.cast_mut(), Ordering::SeqCst); // FIXME: should we add some sort of update counter
                    // FIXME: to determine which update is the most recent?
                    if !old.is_null() {
                        // drop the `virtual reference` we hold to the Arc
                        D::from(old);
                    }
                    break;
                }
            }
        }*/
        if self.try_update(updated) != Some(true) {
            // if the update failed, store it in the update `cache` such that it will be performed
            // later, if possible.
            // FIXME: actually use SwapArcIntermediateTLS's `updated` field in other places in order to perform the `cached` updates, once possible
            self.updated.store(updated.cast_mut(), Ordering::Release);
        }
    }

    unsafe fn try_compare_exchange<const IGNORE_META: bool>(&self, old: *const T, new: D/*&SwapArcIntermediateGuard<'_, T, D>*/) -> Result<Option<bool>, D> {
        // FIXME: what should be compared against? `curr`? or is `update` to be taken into account as well?
        // FIXME: a good solution could be to compare against both `curr` and `intermediate` of the "main struct"(`ArcSwapIntermediateTLSLessFence`)
        // FIXME: this could lead to a problem tho because it doesn't seem very bulletproof to simply compare against 2 values that could point to two entirely different allocations
        // FIXME: if not done with careful consideration this could defeat the whole purpose of implementing compare_exchange for the data structure
        // FIXME: because this could lead to clients of this function to get unreliable feedback and weird updates to be performed which is the worst case scenario and should be avoided at all costs
        // FIXME: another solution could be to let the caller provide whether to check for the intermediate or curr value which is okay because then this
        // FIXME: compare_exchange function would check if the state the caller last saw is still up to date or not which would (probably) solve all problems
        // FIXME: we had before - the only problem that remains is that this will (probably) not work for thread locals because 1.
        // FIXME: their states are inconsistent across different threads and because something loaded as `RefSource::Curr` can become `RefSource::Intermediate`
        // FIXME: without notice - although if the first point could be neglected, the second one can be solved, the only problem remaining is that once
        // FIXME: we compared the thread local states, we have to perform a global update that is propagated down to the source of the compare
        // FIXME: if we do that we have replaced the global state source and have replaced the local one, the only problem remaining still
        // FIXME: is if another thread tries to perform a compare_exchange, but this could be solved by using compare_exchange on the global state source
        // FIXME: and acquiring the update lock. but if the update failed we have to somehow update the local state because otherwise we could
        // FIXME: run into an infinite loop (QUESTION: is this really true? because if we load the most recent value from local, we won't get an expired one -
        // FIXME: the only possible thing that i can think of right now is when intermediate is not null but even then intermediate should always contain
        // FIXME: the most recent value and there shouldn't be a more recent one in global state sources (except for `updated` - but only if that is to be
        // FIXME: considered a "global state source")
        // FIXME: also what is to be considered the "global state source"? is it `curr` or `updated` - or both in some weird way?
        // FIXME: if the local has some `intermediate` value then we can simply use said value for comparison (but only if the provided source is also `intermediate`)
        // FIXME: OR maybe even if not?
        /*if !self.intermediate_ref_cnt.compare_exchange(0, Self::UPDATE | Self::OTHER_UPDATE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return false;
        }
        let intermediate = self.intermediate_ptr.load(Ordering::Acquire);
        let cmp_result = if IGNORE_META {
            Self::strip_metadata(intermediate) == old
        } else {
            intermediate.cast_const() == old
        };
        if !cmp_result {
            self.intermediate_ref_cnt.fetch_and(!(Self::UPDATE | Self::OTHER_UPDATE), Ordering::SeqCst);
            return false;
        }
        // forget `new` in order to create a `virtual reference`
        let new = ManuallyDrop::new(new);
        let new = new.as_ptr();
        // clear out old updates to make sure our update won't be overwritten by them in the future
        let old_update = self.updated.swap(null_mut(), Ordering::SeqCst);
        let metadata = Self::get_metadata(intermediate);
        let new = Self::merge_ptr_and_metadata(new, metadata).cast_mut();
        self.intermediate_ptr.store(new, Ordering::Release);
        // unset the update flag
        self.intermediate_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
        if !old_update.is_null() {
            // drop the `virtual reference` we hold to the Arc
            D::from(old_update);
        }
        match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                let prev = self.ptr.swap(new, Ordering::Release);
                // unset the update flag
                self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                // unset the `weak` update flag from the intermediate ref cnt
                self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst);
                // drop the `virtual reference` we hold to the Arc
                D::from(Self::strip_metadata(prev));
            }
            Err(_) => {}
        }
        true*/
        if IGNORE_META {
            match self.curr.compare_exchange(old.cast_mut(), new.as_ptr().cast_mut().map_addr(|x| x | GLOBAL_UPDATE_FLAG), Ordering::SeqCst, Ordering::Acquire) {
                Ok(_) => {
                    if !self.try_update_locals(old.cast_mut(), new.as_ptr()) {
                        self.curr.fetch_and(!GLOBAL_UPDATE_FLAG, Ordering::Release);
                        return Err(new);
                    }
                    // leak reference to new ptr
                    mem::forget(new);
                    // release the old reference
                    D::from(old);
                    self.curr.fetch_and(!GLOBAL_UPDATE_FLAG, Ordering::Release);
                    Ok(Some(true))
                }
                Err(actual) => {
                    if actual.expose_addr() & GLOBAL_UPDATE_FLAG != 0 {
                        // we can't know whether the new thingy will be the same as the old one or not
                        return Ok(None);
                    }
                    Ok(Some(false))
                }
            }
        } else {
            let curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst);
            if curr.expose_addr() & GLOBAL_UPDATE_FLAG != 0 {
                // we can't know whether the new thingy will be the same as the old one or not
                return Ok(None);
            }
            if curr == old.cast_mut() {
                if !self.try_update_locals(old.cast_mut(), new.as_ptr()) {
                    self.curr.fetch_and(!GLOBAL_UPDATE_FLAG, Ordering::Release);
                    return Err(new);
                }
                // leak reference to new ptr
                mem::forget(new);
                // release the old reference
                D::from(old);
                self.curr.fetch_and(!GLOBAL_UPDATE_FLAG, Ordering::Release);
                Ok(Some(true))
            } else {
                self.curr.fetch_and(!GLOBAL_UPDATE_FLAG, Ordering::Release);
                Ok(Some(false))
            }
        }
    }

    // FIXME: this causes "deadlocks" if there are any other references alive
    unsafe fn try_compare_exchange_with_meta(&self, old: *const T, new: *const T/*&SwapArcIntermediateGuard<'_, T, D>*/) -> bool {
        if !self.intermediate_ref_cnt.compare_exchange(0, Self::UPDATE | Self::OTHER_UPDATE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return false;
        }
        let intermediate = self.intermediate_ptr.load(Ordering::Acquire);
        if intermediate.cast_const() != old {
            self.intermediate_ref_cnt.fetch_and(!(Self::UPDATE | Self::OTHER_UPDATE), Ordering::SeqCst);
            return false;
        }
        // clear out old updates to make sure our update won't be overwritten by them in the future
        let old_update = self.updated.swap(null_mut(), Ordering::SeqCst);
        // increase the ref count
        let tmp = ManuallyDrop::new(D::from(Self::strip_metadata(new)));
        tmp.increase_ref_cnt();
        self.intermediate_ptr.store(new.cast_mut(), Ordering::Release);
        // unset the update flag
        self.intermediate_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
        if !old_update.is_null() {
            // drop the `virtual reference` we hold to the Arc
            D::from(old_update);
        }
        match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                let prev = self.ptr.swap(new.cast_mut(), Ordering::Release);
                // unset the update flag
                self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                // unset the `weak` update flag from the intermediate ref cnt
                self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst);
                // drop the `virtual reference` we hold to the Arc
                D::from(Self::strip_metadata(prev));
            }
            Err(_) => {}
        }
        true
    }

    pub fn update_metadata(&self, metadata: usize) {
        loop {
            let curr = self.curr.load(Ordering::Acquire);
            if curr.expose_addr() & GLOBAL_UPDATE_FLAG == 0 && self.try_update_meta(curr, metadata) { // FIXME: should this be a weak compare_exchange?
                break;
            }
        }
    }

    /// `old` should contain the previous metadata.
    pub fn try_update_meta(&self, old: *const T, metadata: usize) -> bool {
        let prefix = metadata & Self::META_MASK;
        self.curr.compare_exchange(old.cast_mut(), old.map_addr(|x| x | prefix).cast_mut(), Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    pub fn set_in_metadata(&self, active_bits: usize) {
        self.intermediate_ptr.fetch_or(active_bits, Ordering::Release);
    }

    pub fn unset_in_metadata(&self, inactive_bits: usize) {
        self.intermediate_ptr.fetch_and(!inactive_bits, Ordering::Release);
    }

    pub fn load_metadata(&self) -> usize {
        self.intermediate_ptr.load(Ordering::Acquire).expose_addr() & Self::META_MASK
    }

    fn get_metadata(ptr: *const T) -> usize {
        ptr.expose_addr() & Self::META_MASK
    }

    fn strip_metadata(ptr: *const T) -> *const T {
        ptr::from_exposed_addr(ptr.expose_addr() & (!Self::META_MASK))
    }

    fn merge_ptr_and_metadata(ptr: *const T, metadata: usize) -> *const T {
        ptr::from_exposed_addr(ptr.expose_addr() | metadata)
    }

    const META_MASK: usize = {
        let mut result = 0;
        let mut i = 0;
        while METADATA_PREFIX_BITS > i {
            result |= 1 << i;
            i += 1;
        }
        result
    };

    /// This will force an update, this means that new
    /// readers will have to wait for all old readers to
    /// finish and to update the ptr, even when no update
    /// is queued this will block new readers for a short
    /// amount of time, until failure got detected
    fn dummy() {}
    /*fn force_update(&self) -> UpdateResult {
        let curr = self.ref_cnt.fetch_or(Self::FORCE_UPDATE, Ordering::SeqCst);
        if curr & Self::UPDATE != 0 {
            return UpdateResult::AlreadyUpdating;
        }
        if self.updated.load(Ordering::SeqCst).is_null() {
            // unset the flag, as there are no upcoming updates
            self.ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
            return UpdateResult::NoUpdate;
        }
        UpdateResult::Ok
    }*/

    fn load_local(self: &Arc<Self>) -> (&LocalData<T, D, METADATA_PREFIX_BITS>, bool) {
        let mut new = false;
        let parent = self.thread_local.get_or(|| {
            new = true;
            /*LocalData {
                inner: MaybeUninit::new(self.clone().load_internal()),
                ref_cnt: 1,
            }*/
            let mut curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst); // FIXME: is it okay to have a blocking solution here?
            let mut back_off = 1;
            while curr & GLOBAL_UPDATE_FLAG != 0 {
                curr = self.curr.fetch_or(GLOBAL_UPDATE_FLAG, Ordering::SeqCst);
                // back-off
                thread::sleep(Duration::from_micros(10 * back_off));
                back_off += 1;
            }
            // increase the reference count
            let tmp = ManuallyDrop::new(D::from(curr));
            mem::forget(tmp.clone());
            // we know the current state of the ptr stored in curr
            self.curr.fetch_and(GLOBAL_UPDATE_FLAG, Ordering::Release);
            LocalData {
                parent: self.clone(),
                intermediate_update: AtomicPtr::new(curr),
                update: AtomicPtr::new(curr),
                inner: UnsafeCell::new(LocalDataInner {
                    intermediate_ptr: null(),
                    intermediate: LocalCounted { val: MaybeUninit::uninit(), ref_cnt: 0, _phantom_data: Default::default() },
                    new_ptr: null(),
                    new_src: RefSource::Curr,
                    new: LocalCounted { val: MaybeUninit::uninit(), ref_cnt: 0, _phantom_data: Default::default() },
                    // FIXME: increase ref count
                    curr: LocalCounted { val: MaybeUninit::new(D::from(curr)), ref_cnt: 1, _phantom_data: Default::default() }
                }),
            }
        });
        (parent, new)
    }

}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Drop for SwapArcIntermediateTLS<T, D, METADATA_PREFIX_BITS> {
    fn drop(&mut self) {
        // FIXME: how should we handle intermediate inside drop?
        let updated = self.updated.load(Ordering::Acquire);
        if !updated.is_null() {
            D::from(updated);
        }
        let curr = Self::strip_metadata(self.ptr.load(Ordering::Acquire));
        let intermediate = Self::strip_metadata(self.intermediate_ptr.load(Ordering::Acquire));
        if intermediate != curr {
            // FIXME: the reason why we have to do this currently is because the update function doesn't work properly, fix the root cause!
            D::from(intermediate);
        }
        // drop the current arc
        D::from(curr);
    }
}

/*
struct SwapArcIntermediateInternalGuard<T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_PREFIX_BITS: u32 = 0> {
    parent: Arc<SwapArcIntermediateTLS<T, D, METADATA_PREFIX_BITS>>,
    fake_ref: ManuallyDrop<D>,
    ref_src: RefSource,
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Drop for SwapArcIntermediateInternalGuard<T, D, METADATA_PREFIX_BITS> {
    fn drop(&mut self) {
        // release the reference we hold
        match self.ref_src {
            RefSource::Curr => {
                // let ref_cnt = self.parent.curr_ref_cnt.load(Ordering::SeqCst);
                let ref_cnt = self.parent.curr_ref_cnt.fetch_sub(1, Ordering::SeqCst);
                if ref_cnt == 1 {
                    self.parent.try_update_curr();
                }
                // self.parent.curr_ref_cnt.fetch_sub(1, Ordering::SeqCst);
            }
            RefSource::Intermediate => {
                // FIXME: do we actually have to load the ref cnt before subtracting 1 from it?
                // let ref_cnt = self.parent.intermediate_ref_cnt.load(Ordering::SeqCst);
                let ref_cnt = self.parent.intermediate_ref_cnt.fetch_sub(1, Ordering::SeqCst);
                // fast-rejection path to ensure we are only trying to update if it's worth it
                // FIXME: this probably isn't correct: Note: UPDATE is set (seldom) on the immediate ref_cnt if there is a forced update waiting in the queue
                if (ref_cnt == 1/* || ref_cnt == SwapArcIntermediate::<T>::UPDATE*/) && !self.parent.updated.load(Ordering::Acquire).is_null() { // FIXME: does the updated check even help here?
                    self.parent.try_update_intermediate();
                }
                // self.parent.intermediate_ref_cnt.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T> + Display, const METADATA_PREFIX_BITS: u32> Display for SwapArcIntermediateInternalGuard<T, D, METADATA_PREFIX_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        D::fmt(self.fake_ref.deref(), f)
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T> + Debug, const METADATA_PREFIX_BITS: u32> Debug for SwapArcIntermediateInternalGuard<T, D, METADATA_PREFIX_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        D::fmt(self.fake_ref.deref(), f)
    }
}*/

pub struct SwapArcIntermediatePtrGuard<'a, T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_PREFIX_BITS: u32 = 0> {
    parent: &'a LocalData<T, D, METADATA_PREFIX_BITS>,
    ptr: *const T,
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> SwapArcIntermediatePtrGuard<'_, T, D, METADATA_PREFIX_BITS> {

    #[inline]
    pub fn as_raw(&self) -> *const T {
        self.ptr
    }

}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Clone for SwapArcIntermediatePtrGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn clone(&self) -> Self {
        // FIXME: use more recent thingy if available (do a new load)
        // SAFETY: This is safe because we know that we are the only thread that
        // is able to access the thread local data at this time and said data has to be initialized
        // and we also know, that the pointer has to be non-null
        unsafe { self.parent.inner.get().as_mut().unwrap_unchecked() }.ref_cnt += 1;
        SwapArcIntermediatePtrGuard {
            parent: self.parent,
            ptr: self.ptr,
        }
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Drop for SwapArcIntermediatePtrGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn drop(&mut self) {
        // SAFETY: This is safe because we know that we are the only thread that
        // is able to access the thread local data at this time and said data has to be initialized
        // and we also know, that the pointer has to be non-null
        let data = unsafe { self.parent.inner.get().as_mut().unwrap_unchecked() };
        // release the reference we hold
        if unsafe { self.fake_ref.as_ptr() == data.curr.val.assume_init_ref() }.as_ptr() {
            data.curr.ref_cnt -= 1;
            if data.curr.ref_cnt == 0 {
                // SAFETY: This is safe because we know that the reference count
                // was 1 before we just decremented it and thus we know that
                // `inner` has to be initialized right now.
                // unsafe { data.inner.assume_init_drop(); }

                if !data.new_ptr.is_null() {
                    unsafe { data.curr.val.assume_init_drop() };
                    data.curr = mem::take(&mut data.new);
                    if !data.intermediate_ptr.is_null() {
                        data.new = mem::take(&mut data.intermediate);
                        data.new_ptr = data.intermediate_ptr;
                        data.intermediate_ptr = null();
                        self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                    } else {
                        data.new_ptr = null();
                        if data.new_src == RefSource::Curr {
                            self.parent.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                        } else {
                            self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                        }
                    }
                } else {
                    // FIXME: try update!
                    // data.parent.thread_local.iter()
                    // FIXME: go through all thread locals and check if
                    // signal that this thread has no pending updates
                }
            }
        } else if self.fake_ref.as_ptr() == data.new_ptr {
            data.new.ref_cnt -= 1;
            if data.new.ref_cnt == 0 {
                if !data.intermediate_ptr.is_null() {
                    data.new = mem::take(&mut data.intermediate);
                    data.new_ptr = data.intermediate_ptr;
                    data.intermediate_ptr = null();
                    // FIXME: we could optimize the `Arc` ref counting by only increasing and decreasing the ref counts of the thread locals that get used.
                    self.parent.update.store(data.new_ptr.cast_mut(), Ordering::SeqCst);
                    self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                } else {
                    data.new_ptr = null();
                    self.parent.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                }
            }
        } else {
            data.intermediate.ref_cnt -= 1;
            if data.intermediate.ref_cnt == 0 {
                self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
            }
        }
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Debug for SwapArcIntermediatePtrGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let tmp = format!("{:?}", self.ptr);
        f.write_str(tmp.as_str())
    }
}


pub struct SwapArcIntermediateGuard<'a, T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_PREFIX_BITS: u32 = 0> {
    parent: &'a LocalData<T, D, METADATA_PREFIX_BITS>,
    fake_ref: ManuallyDrop<D>,
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Drop for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn drop(&mut self) {
        // SAFETY: This is safe because we know that we are the only thread that
        // is able to access the thread local data at this time and said data has to be initialized
        // and we also know, that the pointer has to be non-null
        let data = unsafe { self.parent.inner.get().as_mut().unwrap_unchecked() };
        // release the reference we hold
        if unsafe { self.fake_ref.as_ptr() == data.curr.val.assume_init_ref() }.as_ptr() {
            data.curr.ref_cnt -= 1;
            if data.curr.ref_cnt == 0 {
                // SAFETY: This is safe because we know that the reference count
                // was 1 before we just decremented it and thus we know that
                // `inner` has to be initialized right now.
                // unsafe { data.inner.assume_init_drop(); }

                if !data.new_ptr.is_null() {
                    unsafe { data.curr.val.assume_init_drop() };
                    data.curr = mem::take(&mut data.new);
                    if !data.intermediate_ptr.is_null() {
                        data.new = mem::take(&mut data.intermediate);
                        data.new_ptr = data.intermediate_ptr;
                        data.intermediate_ptr = null();
                        self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                    } else {
                        data.new_ptr = null();
                        if data.new_src == RefSource::Curr {
                            self.parent.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                        } else {
                            self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                        }
                    }
                } else {
                    // FIXME: try update!
                    // data.parent.thread_local.iter()
                    // FIXME: go through all thread locals and check if
                    // signal that this thread has no pending updates
                }
            }
        } else if self.fake_ref.as_ptr() == data.new_ptr {
            data.new.ref_cnt -= 1;
            if data.new.ref_cnt == 0 {
                if !data.intermediate_ptr.is_null() {
                    data.new = mem::take(&mut data.intermediate);
                    data.new_ptr = data.intermediate_ptr;
                    data.intermediate_ptr = null();
                    // FIXME: we could optimize the `Arc` ref counting by only increasing and decreasing the ref counts of the thread locals that get used.
                    self.parent.update.store(data.new_ptr.cast_mut(), Ordering::SeqCst);
                    self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                } else {
                    data.new_ptr = null();
                    self.parent.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                }
            }
        } else {
            data.intermediate.ref_cnt -= 1;
            if data.intermediate.ref_cnt == 0 {
                self.parent.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
            }
        }
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Deref for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    type Target = D;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.fake_ref.deref()
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Borrow<D> for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    #[inline]
    fn borrow(&self) -> &D {
        self.fake_ref.deref()
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> AsRef<D> for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    #[inline]
    fn as_ref(&self) -> &D {
        self.fake_ref.deref()
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T> + Display, const METADATA_PREFIX_BITS: u32> Display for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        D::fmt(self.as_ref(), f)
    }
}

impl<T: Send + Sync, D: DataPtrConvert<T> + Debug, const METADATA_PREFIX_BITS: u32> Debug for SwapArcIntermediateGuard<'_, T, D, METADATA_PREFIX_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        D::fmt(self.as_ref(), f)
    }
}

enum RefSource {
    Curr,
    Intermediate,
}

/*
const STATE_IDLING: u8 =  0b00; // 0
const STATE_UPDATED: u8 = 0b10; // 1
const STATE_USING: u8 =   0b01; // 2
*/
const STATE_READY: u8 = 0b000;     // 0 - this state indicates that an update can be performed
const STATE_IN_USE: u8 = 0b100;    // 1 - this state indicates that there is an update being performed
const STATE_UPDATABLE: u8 = 0b010; // 2 - this state indicates that there is a pending update
const STATE_UPDATING: u8 = 0b011;  // 6 - this state indicates that there is an external update being performed - this is intentionally the same value as
                                   // the `NORMALIZATION_MASK`
const NORMALIZATION_MASK: u8 = 0b011; // can be used to make all states equal except the in_use state

const IN_USE_FLAG: usize = 1 << 0;
const UPDATE_FLAG: usize = 1 << 1;
const META_DATA_MASK: usize = IN_USE_FLAG | UPDATE_FLAG;

#[derive(Default)]
struct LocalData<T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_PREFIX_BITS: u32 = 0> {
    parent: Arc<SwapArcIntermediateTLS<T, D, METADATA_PREFIX_BITS>>,
    intermediate_update: AtomicPtr<T>,
    update: AtomicPtr<T>,
    inner: UnsafeCell<LocalDataInner<T, D, METADATA_PREFIX_BITS>>,
}

impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> LocalData<T, D, METADATA_PREFIX_BITS> {

    fn load_update(&self) -> (*mut T, RefSource) {
        let update = self.update.fetch_or(IN_USE_FLAG, Ordering::SeqCst);
        let (ptr, src) = if update.expose_addr() & UPDATE_FLAG != 0 {
            let intermediate = self.intermediate_update.fetch_or(IN_USE_FLAG, Ordering::SeqCst);
            /*if intermediate.expose_addr() & UPDATE_FLAG != 0 {
                let update = self.update.load(Ordering::Acquire);
                // release the redundant reference
                self.intermediate_update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                (update.map_addr(|x| x & !META_DATA_MASK), RefSource::Curr)
            } else {
                // release the redundant reference
                self.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
                (intermediate.map_addr(|x| x & !META_DATA_MASK), RefSource::Intermediate)
            }*/
            // release the redundant reference
            self.update.fetch_and(!IN_USE_FLAG, Ordering::SeqCst);
            (intermediate.map_addr(|x| x & !META_DATA_MASK), RefSource::Intermediate)
        } else {
            (update.map_addr(|x| x & !META_DATA_MASK), RefSource::Curr)
        };
        (ptr, src)
    }

    /*
    fn update(&self, new: *mut T) -> bool {
        let updated = Self::strip_metadata(updated);
        loop {
            match self.intermediate_ref_cnt.compare_exchange(0, Self::UPDATE | Self::OTHER_UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    let new = updated.cast_mut();
                    // clear out old updates to make sure our update won't be overwritten by them in the future
                    let old = self.updated.swap(null_mut(), Ordering::SeqCst);
                    let metadata = Self::get_metadata(self.intermediate_ptr.load(Ordering::Acquire));
                    let new = Self::merge_ptr_and_metadata(new, metadata).cast_mut();
                    self.intermediate_ptr.store(new, Ordering::Release);
                    // unset the update flag
                    self.intermediate_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                    if !old.is_null() {
                        // drop the `virtual reference` we hold to the Arc
                        D::from(old);
                    }
                    // try finishing the update up!
                    match self.curr_ref_cnt.compare_exchange(0, Self::UPDATE, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => {
                            let prev = self.ptr.swap(new, Ordering::Release);
                            // unset the update flag
                            self.curr_ref_cnt.fetch_and(!Self::UPDATE, Ordering::SeqCst);
                            // unset the `weak` update flag from the intermediate ref cnt
                            self.intermediate_ref_cnt.fetch_and(!Self::OTHER_UPDATE, Ordering::SeqCst);
                            // drop the `virtual reference` we hold to the Arc
                            D::from(Self::strip_metadata(prev));
                        }
                        Err(_) => {}
                    }
                    break;
                }
                Err(old) => {
                    if old & Self::UPDATE != 0 { // FIXME: what about Self::UPDATE_OTHER?
                        // somebody else already updates the current ptr, so we wait until they finish their update
                        continue;
                    }
                    // push our update up, so it will be applied in the future
                    let old = self.updated.swap(updated.cast_mut(), Ordering::SeqCst); // FIXME: should we add some sort of update counter
                    // FIXME: to determine which update is the most recent?
                    if !old.is_null() {
                        // drop the `virtual reference` we hold to the Arc
                        D::from(old);
                    }
                    break;
                }
            }
        }
    }*/

}

struct LocalDataInner<T: Send + Sync, D: DataPtrConvert<T> = Arc<T>, const METADATA_PREFIX_BITS: u32 = 0> {
    intermediate_ptr: *const T,
    intermediate: LocalCounted<D>,
    new_ptr: *const T,
    new_src: RefSource,
    new: LocalCounted<D>,
    curr: LocalCounted<D>,
}

unsafe impl<T: Send + Sync, D: DataPtrConvert<T>, const METADATA_PREFIX_BITS: u32> Send for LocalDataInner<T, D, METADATA_PREFIX_BITS> {}

struct LocalCounted<T: Send + Sync, D: DataPtrConvert<T> = Arc<T>> {
    val: MaybeUninit<D>,
    ref_cnt: usize,
    _phantom_data: PhantomData<T>,
}

/// SAFETY: Types implementing this trait are expected to perform
/// reference counting through cloning/dropping internally.
pub unsafe trait RefCnt: Send + Sync + Sync + Clone {}

pub trait DataPtrConvert<T>: RefCnt + Sized {

    const INVALID: *const T;

    /// This function may not alter the reference count of the
    /// reference counted "object".
    fn from(ptr: *const T) -> Self;

    /// This function should decrement the reference count of the
    /// reference counted "object" indirectly, by automatically
    /// decrementing it on drop inside the "object"'s drop
    /// implementation.
    fn into(self) -> *const T;

    /// This function should NOT decrement the reference count of the
    /// reference counted "object" in any way, shape or form.
    fn as_ptr(&self) -> *const T;

    /// This function should increment the reference count of the
    /// reference counted "object" directly.
    fn increase_ref_cnt(&self);

}

unsafe impl<T: Send + Sync> RefCnt for Arc<T> {}

impl<T: Send + Sync> DataPtrConvert<T> for Arc<T> {
    const INVALID: *const T = null();

    fn from(ptr: *const T) -> Self {
        unsafe { Arc::from_raw(ptr) }
    }

    fn into(self) -> *const T {
        let ret = Arc::into_raw(self);
        // decrement the reference count
        <Self as DataPtrConvert<T>>::from(ret);
        ret
    }

    fn as_ptr(&self) -> *const T {
        Arc::as_ptr(self)
    }

    fn increase_ref_cnt(&self) {
        mem::forget(self.clone());
    }
}

unsafe impl<T: Send + Sync> RefCnt for Option<Arc<T>> {}

impl<T: Send + Sync> DataPtrConvert<T> for Option<Arc<T>> {
    const INVALID: *const T = null();

    fn from(ptr: *const T) -> Self {
        if !ptr.is_null() {
            Some(unsafe { Arc::from_raw(ptr) })
        } else {
            None
        }
    }

    fn into(self) -> *const T {
        match self {
            None => null(),
            Some(val) => {
                let ret = Arc::into_raw(val);
                // decrement the reference count
                <Self as DataPtrConvert<T>>::from(ret);
                ret
            },
        }
    }

    fn as_ptr(&self) -> *const T {
        match self {
            None => null(),
            Some(val) => Arc::as_ptr(val),
        }
    }

    fn increase_ref_cnt(&self) {
        mem::forget(self.clone());
    }
}
*/
