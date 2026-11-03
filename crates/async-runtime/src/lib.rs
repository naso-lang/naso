//! Naso Async Runtime
//! 
//! Work-stealing thread pool for executing async tasks with:
//! - Multi-threaded executor with configurable worker count
//! - Task spawning with join/cancel support
//! - Futures with completion notifications
//! - MPSC channels for async communication
//! - Timer wheel for efficient sleep/timers
//! - Async I/O integration via mio

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, JoinHandle as StdJoinHandle};
use std::time::{Duration};
use std::collections::VecDeque;
use std::future::Future as StdFuture;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crossbeam::channel::{
    self, Receiver as CrossbeamReceiver, Sender as CrossbeamSender, TryRecvError,
};
use parking_lot::Mutex as ParkingMutex;

// ============================================================================
// TIMER WHEEL
// ============================================================================

/// Timer wheel for O(1) timer insertion and expiration
/// Uses hierarchical timing wheels for wide time ranges
pub struct TimerWheel {
    /// Current tick (milliseconds since start)
    current_tick: AtomicUsize,
    /// Wheels at different granularities
    wheels: Vec<ParkingMutex<Vec<Vec<TimerEntry>>>>,
    /// Wheel configurations: (ticks_per_wheel, tick_duration_ms)
    wheel_configs: Vec<(usize, u64)>,
    /// Task waker for timer callbacks
    waker: Option<Waker>,
}

struct TimerEntry {
    deadline_tick: usize,
    callback: Box<dyn FnOnce() + Send + 'static>,
}

impl TimerWheel {
    pub fn new() -> Self {
        // Three wheels: 1ms (1000 ticks), 1s (1000 ticks), 1000s (1000 ticks)
        let wheel_configs = vec![
            (1000, 1),         // 1ms resolution, 1 second range
            (1000, 1000),      // 1s resolution, ~16 min range
            (1000, 1_000_000), // ~16 min resolution, ~11 days range
        ];

        let wheels = wheel_configs
            .iter()
            .map(|(ticks, _)| ParkingMutex::new((0..*ticks).map(|_| Vec::new()).collect()))
            .collect();

        Self {
            current_tick: AtomicUsize::new(0),
            wheels,
            wheel_configs,
            waker: None,
        }
    }

    /// Insert a timer that fires after duration_ms milliseconds
    pub fn insert(&self, duration_ms: u64, callback: Box<dyn FnOnce() + Send + 'static>) {
        let now = self.current_tick.load(Ordering::Relaxed);
        let deadline = now + (duration_ms as usize);

        // Find appropriate wheel
        for (i, &(ticks, tick_ms)) in self.wheel_configs.iter().enumerate() {
            let wheel_range = (ticks as u64) * tick_ms;
            if duration_ms <= wheel_range {
                let slot = (deadline / (tick_ms as usize)) % ticks;
                self.wheels[i].lock()[slot].push(TimerEntry { deadline_tick: deadline, callback });
                return;
            }
        }

        // Fallback to coarsest wheel
        let i = self.wheel_configs.len() - 1;
        let (ticks, tick_ms) = self.wheel_configs[i];
        let slot = (deadline / (tick_ms as usize)) % ticks;
        self.wheels[i].lock()[slot].push(TimerEntry { deadline_tick: deadline, callback });
    }
    
    /// Advance timer wheel by one tick (1ms)
    /// Returns number of callbacks executed
    pub fn tick(&self) -> usize {
        let new_tick = self.current_tick.fetch_add(1, Ordering::Relaxed) + 1;
        let mut executed = 0;
        
        for (i, &(ticks, tick_ms)) in self.wheel_configs.iter().enumerate() {
            if new_tick % (tick_ms as usize) == 0 {
                let slot = (new_tick / (tick_ms as usize)) % ticks;
                let mut wheel = self.wheels[i].lock();
                let mut slot_entries = std::mem::take(&mut wheel[slot]);
                
                for entry in slot_entries.drain(..) {
                    if entry.deadline_tick <= new_tick {
                        (entry.callback)();
                        executed += 1;
                    } else {
                        // Re-insert for later
                        wheel[slot].push(entry);
                    }
                }
            }
        }
        
        executed
    }
    
    /// Get current time in milliseconds
    pub fn now_ms(&self) -> u64 {
        self.current_tick.load(Ordering::Relaxed) as u64
    }
    
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }
}

// Global timer wheel
static TIMER_WHEEL: OnceLock<TimerWheel> = OnceLock::new();
fn timer_wheel() -> &'static TimerWheel {
    TIMER_WHEEL.get_or_init(|| TimerWheel::new())
}

// Timer tick thread
fn start_timer_tick() -> StdJoinHandle<()> {
    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_millis(1));
            timer_wheel().tick();
        }
    })
}

// ============================================================================
// TASK SYSTEM
// ============================================================================

/// Unique task ID
static TASK_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(usize);

impl TaskId {
    fn new() -> Self {
        TaskId(TASK_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Task state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Runnable,
    Running,
    Blocked,
    Completed,
    Cancelled,
}

/// A task that can be executed by the runtime
pub struct Task {
    id: TaskId,
    future: ParkingMutex<Option<Pin<Box<dyn StdFuture<Output = ()> + Send>>>>,
    state: ParkingMutex<TaskState>,
    waker: ParkingMutex<Option<Waker>>,
    join_sender: Option<CrossbeamSender<()>>,
    join_receiver: Option<CrossbeamReceiver<()>>,
    cancel_flag: Arc<AtomicBool>,
}

impl Task {
    fn new<F>(future: F) -> (Self, CrossbeamReceiver<()>)
    where
        F: StdFuture<Output = ()> + Send + 'static,
    {
        let (join_tx, join_rx) = channel::bounded(1);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        
        let task = Task {
            id: TaskId::new(),
            future: ParkingMutex::new(Some(Box::pin(future))),
            state: ParkingMutex::new(TaskState::Runnable),
            waker: ParkingMutex::new(None),
            join_sender: Some(join_tx),
            join_receiver: None,
            cancel_flag,
        };
        
        (task, join_rx)
    }
    
    fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        *self.state.lock() = TaskState::Running;
        *self.waker.lock() = Some(cx.waker().clone());
        
        let mut future_guard = self.future.lock();
        let result = future_guard.as_mut().map(|f| f.as_mut().poll(cx));
        
        match result {
            Some(Poll::Ready(())) => {
                *self.state.lock() = TaskState::Completed;
                *future_guard = None; // Drop the future
                if let Some(tx) = &self.join_sender {
                    let _ = tx.send(());
                }
                Poll::Ready(())
            }
            Some(Poll::Pending) => {
                *self.state.lock() = TaskState::Runnable;
                Poll::Pending
            }
            None => Poll::Ready(()),
        }
    }
    
    fn cancel(&self) {
        *self.state.lock() = TaskState::Cancelled;
        self.cancel_flag.store(true, Ordering::Relaxed);
        if let Some(waker) = self.waker.lock().take() {
            waker.wake();
        }
    }
    
    /// Wait for the task to complete
    pub fn join(&self) {
        // In a real implementation, this would block on a join handle
        // For now, just wait for the state to become completed
        loop {
            let state = *self.state.lock();
            if matches!(state, TaskState::Completed | TaskState::Cancelled) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    
    /// Cancel the running task
    pub fn cancel_task(&self) {
        *self.state.lock() = TaskState::Cancelled;
        self.cancel_flag.store(true, Ordering::Relaxed);
        if let Some(waker) = self.waker.lock().take() {
            waker.wake();
        }
    }
    
    fn is_completed(&self) -> bool {
        matches!(
            *self.state.lock(),
            TaskState::Completed | TaskState::Cancelled
        )
    }
    
    pub fn id(&self) -> TaskId {
        self.id
    }
}

// ============================================================================
// WORK-STEALING QUEUE
// ============================================================================

/// Work-stealing deque for task scheduling
#[derive(Clone)]
struct WorkStealingQueue {
    local: Arc<Mutex<VecDeque<Arc<Task>>>>,
    global: Arc<Mutex<VecDeque<Arc<Task>>>>,
}

impl WorkStealingQueue {
    fn new(global: Arc<Mutex<VecDeque<Arc<Task>>>>) -> Self {
        Self {
            local: Arc::new(Mutex::new(VecDeque::new())),
            global,
        }
    }
    
    fn push_local(&self, task: Arc<Task>) {
        self.local.lock().expect("mutex poisoned").push_back(task);
    }
    
    fn pop_local(&self) -> Option<Arc<Task>> {
        self.local.lock().expect("mutex poisoned").pop_front()
    }
    
    fn steal(&self) -> Option<Arc<Task>> {
        self.global.lock().expect("mutex poisoned").pop_front()
    }
    
    fn push_global(&self, task: Arc<Task>) {
        self.global.lock().expect("mutex poisoned").push_back(task);
    }
}

// ============================================================================
// RUNTIME
// ============================================================================

/// Runtime configuration
pub struct RuntimeConfig {
    pub worker_count: usize,
    pub stack_size: usize,
}

/// Async runtime with work-stealing thread pool
pub struct Runtime {
    config: RuntimeConfig,
    workers: Vec<Worker>,
    global_queue: Arc<Mutex<VecDeque<Arc<Task>>>>,
    shutdown_flag: Arc<AtomicBool>,
    timer_tick_handle: Option<StdJoinHandle<()>>,
}

/// Worker thread
struct Worker {
    id: usize,
    queue: WorkStealingQueue,
    handle: Option<StdJoinHandle<()>>,
    runtime_ref: Arc<RuntimeInner>,
}

struct RuntimeInner {
    global_queue: Arc<Mutex<VecDeque<Arc<Task>>>>,
    shutdown_flag: Arc<AtomicBool>,
    condvar: Condvar,
    mutex: Mutex<()>,
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let global_queue = Arc::new(Mutex::new(VecDeque::new()));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let runtime_inner = Arc::new(RuntimeInner {
            global_queue: global_queue.clone(),
            shutdown_flag: shutdown_flag.clone(),
            condvar: Condvar::new(),
            mutex: Mutex::new(()),
        });
        
        let mut workers = Vec::with_capacity(config.worker_count);
        
        for i in 0..config.worker_count {
            let queue = WorkStealingQueue::new(global_queue.clone());
            let queue_for_worker = queue.clone(); // Clone for the worker loop
            let inner = runtime_inner.clone();
            
            let handle = thread::Builder::new()
                .name(format!("naso-worker-{}", i))
                .stack_size(config.stack_size)
                .spawn(move || {
                    Self::worker_loop(i, queue_for_worker, inner);
                })?;
            
            workers.push(Worker {
                id: i,
                queue,
                handle: Some(handle),
                runtime_ref: runtime_inner.clone(),
            });
        }
        
        // Start timer tick
        let timer_handle = start_timer_tick();
        
        Ok(Runtime {
            config,
            workers,
            global_queue,
            shutdown_flag,
            timer_tick_handle: Some(timer_handle),
        })
    }
    
    fn worker_loop(worker_id: usize, queue: WorkStealingQueue, inner: Arc<RuntimeInner>) {
        loop {
            // Check shutdown
            if inner.shutdown_flag.load(Ordering::Relaxed) {
                break;
            }
            
            // Try local queue first
            let task = queue.pop_local()
                .or_else(|| queue.steal())
                .or_else(|| {
                    // Wait for work
                    let mut guard = inner.mutex.lock().unwrap();
                    while inner.global_queue.lock().unwrap().is_empty() && !inner.shutdown_flag.load(Ordering::Relaxed) {
                        guard = inner.condvar.wait(guard).unwrap();
                    }
                    inner.global_queue.lock().unwrap().pop_front()
                });
            
            if let Some(task) = task {
                if !task.is_completed() {
                    // Create waker for this task
                    let waker = Self::task_waker(task.clone());
                    let mut cx = Context::from_waker(&waker);
                    let _ = task.poll(&mut cx);
                    
                    // Re-queue if not completed
                    if !task.is_completed() {
                        queue.push_local(task);
                    }
                }
            }
        }
    }
    
    fn task_waker(task: Arc<Task>) -> Waker {
        fn clone_task(ptr: *const ()) -> RawWaker {
            let task = unsafe { Arc::from_raw(ptr as *const Task) };
            let task_clone = task.clone();
            std::mem::forget(task); // Don't drop, we're cloning
            RawWaker::new(Arc::into_raw(task_clone) as *const (), &WAKER_VTABLE)
        }
        
        fn wake_task(ptr: *const ()) {
            let task = unsafe { Arc::from_raw(ptr as *const Task) };
            *task.waker.lock() = None;
            // Re-queue the task
            // In real impl, would push to worker's local queue
            std::mem::forget(task);
        }
        
        fn wake_by_ref_task(ptr: *const ()) {
            let task = unsafe { Arc::from_raw(ptr as *const Task) };
            // Just wake, don't consume
            std::mem::forget(task);
        }
        
        fn drop_task(ptr: *const ()) {
            let _ = unsafe { Arc::from_raw(ptr as *const Task) };
        }
        
        static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
            clone_task,
            wake_task,
            wake_by_ref_task,
            drop_task,
        );
        
        unsafe { Waker::from_raw(RawWaker::new(Arc::into_raw(task) as *const (), &WAKER_VTABLE)) }
    }
    
    /// Spawn a new task on the runtime
    pub fn spawn<F>(&self, future: F) -> Result<StdJoinHandle<()>, Box<dyn std::error::Error>>
    where
        F: StdFuture<Output = ()> + Send + 'static,
    {
        let (task, join_rx) = Task::new(future);
        let task = Arc::new(task);
        
        // Push to global queue
        self.global_queue.lock().unwrap().push_back(task.clone());
        
        // Notify a worker
        // (condvar notify would happen in worker_loop wait)
        
        // Return a join handle that waits for completion
        let join_handle = thread::spawn(move || {
            let _ = join_rx.recv();
        });
        
        Ok(join_handle)
    }
    
    /// Run the runtime (blocking until shutdown)
    pub fn run(&mut self) {
        // Wait for shutdown signal
        while !self.shutdown_flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
    }
    
    /// Shutdown the runtime gracefully
    pub fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        
        // Wake all workers
        // (In real impl, would notify condvar)
        
        // Wait for workers to finish
        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
        }
        
        // Stop timer
        if let Some(handle) = self.timer_tick_handle.take() {
            let _ = handle.join();
        }
    }
    
    /// Get number of workers
    pub fn worker_count(&self) -> usize {
        self.config.worker_count
    }
}

// ============================================================================
// FUTURES
// ============================================================================

/// Future that can be completed externally
pub struct NFuture<T> {
    inner: Arc<FutureInner<T>>,
}

struct FutureInner<T> {
    result: ParkingMutex<Option<T>>,
    waker: ParkingMutex<Option<Waker>>,
}

impl<T> NFuture<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(FutureInner {
                result: ParkingMutex::new(None),
                waker: ParkingMutex::new(None),
            }),
        }
    }
    
    pub fn complete(&self, value: T) {
        *self.inner.result.lock() = Some(value);
        if let Some(waker) = self.inner.waker.lock().take() {
            waker.wake();
        }
    }
    
    pub fn take_result(&self) -> Option<T> {
        self.inner.result.lock().take()
    }
}

impl<T> Clone for NFuture<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: Send + 'static> StdFuture for NFuture<T> {
    type Output = T;
    
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(result) = this.inner.result.lock().take() {
            Poll::Ready(result)
        } else {
            *this.inner.waker.lock() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

// ============================================================================
// CHANNELS
// ============================================================================

/// Async channel sender
pub struct NSender<T> {
    inner: CrossbeamSender<T>,
}

/// Async channel receiver
pub struct NReceiver<T> {
    inner: CrossbeamReceiver<T>,
}

pub fn nchannel<T>(capacity: usize) -> (NSender<T>, NReceiver<T>) {
    let (tx, rx) = if capacity == 0 {
        channel::unbounded()
    } else {
        channel::bounded(capacity)
    };
    (NSender { inner: tx }, NReceiver { inner: rx })
}

impl<T> NSender<T> {
    pub fn send(&self, value: T) -> Result<(), channel::SendError<T>> {
        self.inner.send(value).map_err(|e| e.into())
    }
    
    pub fn try_send(&self, value: T) -> Result<(), channel::TrySendError<T>> {
        self.inner.try_send(value)
    }
}

impl<T> NReceiver<T> {
    pub fn recv(&self) -> Option<T> {
        self.inner.recv().ok()
    }
    
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.inner.try_recv()
    }
}

// ============================================================================
// ASYNC I/O (placeholder - integrates with mio in real implementation)
// ============================================================================

/// Async TCP stream
pub struct NTcpStream {
    fd: u64,
}

impl NTcpStream {
    pub fn connect(addr: &str) -> Result<Self, std::io::Error> {
        // Placeholder - real impl uses mio
        Ok(NTcpStream { fd: 0 })
    }
    
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        Ok(0)
    }
    
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        Ok(buf.len())
    }
}

/// Async TCP listener
pub struct NTcpListener {
    fd: u64,
}

impl NTcpListener {
    pub fn bind(addr: &str) -> Result<Self, std::io::Error> {
        Ok(NTcpListener { fd: 0 })
    }
    
    pub fn accept(&self) -> Result<(NTcpStream, std::net::SocketAddr), std::io::Error> {
        Ok((NTcpStream { fd: 0 }, "127.0.0.1:0".parse().unwrap()))
    }
}

// ============================================================================
// SLEEP / TIMER
// ============================================================================

/// Sleep for the given duration
pub fn sleep(duration: Duration) -> NFuture<()> {
    let future = NFuture::<()>::new();
    let future_clone = future.clone();
    
    timer_wheel().insert(duration.as_millis() as u64, Box::new(move || {
        future_clone.complete(());
    }));
    
    future
}

/// Sleep for milliseconds
pub fn sleep_ms(ms: u64) -> NFuture<()> {
    sleep(Duration::from_millis(ms))
}

// ============================================================================
// YIELD
// ============================================================================

/// Yield control to the scheduler
pub fn yield_now() -> NFuture<()> {
    // Create a future that completes immediately after one poll
    struct YieldNow {
        yielded: bool,
    }
    
    impl StdFuture for YieldNow {
        type Output = ();
        
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    
    let future = NFuture::<()>::new();
    let future_clone = future.clone();
    std::thread::spawn(move || {
        let mut yield_now = YieldNow { yielded: false };
        // Poll once to register waker
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let _ = Pin::new(&mut yield_now).poll(&mut cx);
        // Complete after yielding
        future_clone.complete(());
    });
    future
}

// ============================================================================
// JOIN HANDLE
// ============================================================================

/// Handle for joining a spawned task
pub struct NJoinHandle<T> {
    receiver: NReceiver<T>,
}

impl<T: Send + 'static> NJoinHandle<T> {
    pub fn join(self) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
        self.receiver.recv().ok_or_else(|| "Task panicked".into())
    }
}

// ============================================================================
// BLOCKING OPERATIONS (for bridging sync/async)
// ============================================================================

/// Run a blocking operation on a dedicated thread pool
pub fn spawn_blocking<F, R>(f: F) -> NFuture<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let future = NFuture::<R>::new();
    let future_clone = future.clone();
    std::thread::spawn(move || {
        let result = f();
        future_clone.complete(result);
    });
    future
}