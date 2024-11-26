use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;

pub trait ReportingMultiWrapper {
	fn new() -> Self;
	// one could provide an implementation if you can get the list of bars,
	// but we can't assume we can, so, don't.
	fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R;
}

// represents a generic progress reporting struct
pub trait Reporter {
	fn new(msg: &str) -> Self;
	// screw you, non-mut references, build a wrapper with mutex if its an issue.
	fn incr(&self, n: usize);
	fn count(&self) -> usize;
	fn tick(&self);
	fn done_clear(&self);
	fn done(&self);
	fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R;
}

// a progress reporter that has a size, eg a bar
pub trait ReporterSized: Reporter {
	fn new(msg: &str, len: usize) -> Self;
	fn set_len(&self, len: usize);
	fn length(&self) -> usize;
}

pub trait CanBeWrappedBy<W: ReportingMultiWrapper> : Reporter {
	fn add_to(self, w: &W) -> Self;
}

pub(crate) struct AutoSpin<'a, R: Reporter+Sync> {
	run: Box<AtomicBool>,
	jh: MaybeUninit<JoinHandle<()>>,
	rep: &'a R, // stored exclusively for all_good().
	//_ph: PhantomData<&'a R>,
}

impl<'a, R: Reporter+Sync> AutoSpin<'a, R> {
	pub fn spin(r: &'a R) -> Self {
		// construct self
		let mut s = Self {
			// box so the ptr never moves
			run: Box::new(AtomicBool::new(true)),
			rep: r,
			jh: MaybeUninit::zeroed(),
			//_ph: PhantomData::default()
		};

		// man wtf
		// i hate this language some of the time. not most of it, but some of it.
		// i'll be looking into a less ass way of doing this.
		// i am very sure this is safe, i just don't know how to convince rustc of that.
		// i really could do with a version of std::thread::scope that used a struct's scope or something
		let run_ptr = std::ptr::from_mut(s.run.as_mut()) as usize;
		let rep_ptr = std::ptr::from_ref(r) as usize;

		s.jh.write(thread::spawn(move || {
			let run_ptr = run_ptr as *mut AtomicBool;
			let rep_ptr = rep_ptr as *const R;

			while unsafe { (*run_ptr).load(Ordering::Acquire) } {
				unsafe { &*rep_ptr }.tick();
				thread::sleep_ms(50);
			}
		}));

		s
	}

	/// finishes autospinning then calls done() on the internal object.
	/// mainly useful to extend the lifetime of autospin in a neater way than explicit drop().
	pub fn all_good(self) {
		self.rep.done();
		// self drops here to finish autospinning
	}
}

impl<'a, R: Reporter+Sync> Drop for AutoSpin<'a, R> {
	fn drop(&mut self) {
		// tell the thread to stop
		self.run.store(false, Ordering::Release);
		// wait for it to stop
		let uninit = std::mem::replace(&mut self.jh, MaybeUninit::zeroed());
		unsafe { uninit.assume_init() }.join().unwrap();
	}
}
