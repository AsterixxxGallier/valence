use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use evenio::prelude::*;
use parking_lot::{Condvar, Mutex};

type Task = Box<dyn FnOnce(&mut World)>;

/// Resource component used for deferring the execution of tasks.
///
/// # Examples
///
/// Basic usage:
///
/// ```
/// # use std::time::{Duration, Instant};
/// # use evenio::prelude::*;
/// # use valence_server::scheduler::{run_scheduler, Scheduler};
/// #
/// # #[derive(GlobalEvent)]
/// # struct MyEvent;
/// #
/// // Create the world and scheduler.
/// let mut world = World::new();
/// world.spawn(Scheduler::new());
///
/// // Schedule a task.
/// let scheduler = world.single_mut::<&mut Scheduler>().unwrap();
/// scheduler.schedule(Instant::now() + Duration::from_secs(1), |world| {
///     // Send an event. This will run after one second has passed.
///     world.send(MyEvent);
/// });
///
/// // Listen for the sent event.
/// world.add_handler(|_: Receiver<MyEvent>| {
///     println!("Event received!");
/// });
///
/// // Run the scheduler. This will return after all tasks have run.
/// run_scheduler(&mut world);
/// ```
#[derive(Component, Default)]
pub struct Scheduler {
    tasks: BTreeMap<Instant, Task>,
    interrupt_handle: InterruptHandle,
    // The scheduler should not move between threads. This anchor field ensures
    // that Scheduler does not implement Send or Sync.
    _anchor: PhantomData<Rc<()>>,
}

impl Scheduler {
    /// Returns a new scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedule a task to be run at a particular point in time. The task
    /// will not be run before this instant is reached, and as soon as
    /// possible afterward. Note that the scheduler has to be [run] for the
    /// task to be executed.
    ///
    /// [run]: run_scheduler
    pub fn schedule(&mut self, at: Instant, f: impl FnOnce(&mut World) + 'static) {
        self.tasks.insert(at, Box::new(f));
    }

    /// Returns a handle that can be used to interrupt the thread that the
    /// scheduler is run in.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        self.interrupt_handle.clone()
    }
}

#[derive(Default)]
struct InterruptHandleInner {
    interrupted: Mutex<bool>,
    condvar: Condvar,
}

/// Handle that can be used to interrupt the thread the scheduler is being
/// run in. Refer to the documentation of [`Scheduler`] and
/// [`run_scheduler`] for more information.
#[derive(Default, Clone)]
pub struct InterruptHandle {
    inner: Arc<InterruptHandleInner>,
}

impl InterruptHandle {
    /// Interrupts the thread in which the scheduler is being run. Refer to
    /// the documentation of [`Scheduler`] and [`run_scheduler`] for more
    /// information.
    pub fn interrupt_now(&self) {
        *self.inner.interrupted.lock() = true;
        self.inner.condvar.notify_one();
    }
}

/// Runs all scheduled tasks at approximately the time they were scheduled
/// for. Returns when no tasks are left to run or the function has been
/// interrupted by a different thread using the handle returned by
/// [`Scheduler::interrupt_handle`].
///
/// # Panics
///
/// Panics if there is not exactly one entity with the [`Scheduler`]
/// component.
///
/// [`set_thread`]: Scheduler::set_thread
pub fn run_scheduler(world: &mut World) {
    loop {
        // Obtain a mutable reference to the world's scheduler.
        let scheduler = world
            .single_mut::<&mut Scheduler>()
            .expect("there should be exactly one entity with the Scheduler component");

        // Get the task that needs to run next.
        let Some((at, task)) = scheduler.tasks.pop_first() else {
            break;
        };

        // Wait until the scheduled execution time is reached or the thread
        // is interrupted.
        let handle_inner = &*scheduler.interrupt_handle.inner;
        let mut interrupted = handle_inner.interrupted.lock();
        let result = handle_inner.condvar.wait_until(&mut interrupted, at);
        drop(interrupted);

        if result.timed_out() {
            // Run the task.
            task(world);
        } else {
            // We were interrupted. Break the loop.
            break;
        }
    }
}