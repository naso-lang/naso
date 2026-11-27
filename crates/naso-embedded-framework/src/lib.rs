//! Naso Embedded Framework
//!
//! Embedded systems framework for Naso with:
//! - Hardware Abstraction Layer (HAL)
//! - Real-time operating system support (RTIC)
//! - Device drivers (GPIO, I2C, SPI, UART, ADC, PWM)
//! - Interrupt handling
//! - Power management
//! - No-std support

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use critical_section::Mutex;
use heapless::Vec;
use thiserror::Error;

pub mod hal;
pub mod drivers;
pub mod rtos;
pub mod power;
pub mod interrupts;
pub mod memory;
pub mod cli;
pub mod build;

use hal::gpio::Gpio;
use hal::i2c::I2c;
use hal::spi::Spi;
use hal::uart::Uart;
use hal::adc::Adc;
use hal::pwm::Pwm;

static SYSTEM_READY: AtomicBool = AtomicBool::new(false);

#[derive(Error, Debug)]
pub enum EmbedError {
    #[error("HAL error: {0}")]
    Hal(String),
    #[error("Driver error: {0}")]
    Driver(String),
    #[error("Bus error: {0}")]
    Bus(String),
    #[error("Timeout")]
    Timeout,
    #[error("Invalid pin: {0}")]
    InvalidPin(u8),
    #[error("Not initialized")]
    NotInitialized,
    #[error("Already initialized")]
    AlreadyInitialized,
    #[error("Resource busy")]
    ResourceBusy,
    #[error("Unsupported architecture")]
    UnsupportedArch,
}

pub type Result<T> = core::result::Result<T, EmbedError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMode {
    Input,
    Output,
    Alternate { function: u8 },
    Analog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinPull {
    None,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinSpeed {
    Low,
    Medium,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub port: u8,
    pub number: u8,
}

impl Pin {
    pub fn new(port: u8, number: u8) -> Self {
        Self { port, number }
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}{}", self.port, self.number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    Internal,
    External { hz: u32 },
    Pll { source: Box<ClockSource>, multiplier: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemClock {
    pub hclk_hz: u32,
    pub pclk1_hz: u32,
    pub pclk2_hz: u32,
}

impl SystemClock {
    pub fn default() -> Self {
        Self {
            hclk_hz: 168_000_000,
            pclk1_hz: 42_000_000,
            pclk2_hz: 84_000_000,
        }
    }
}

pub struct System {
    clock: SystemClock,
    gpio: Option<Gpio>,
    i2c: Option<I2c>,
    spi: Option<Spi>,
    uart: Option<Uart>,
    adc: Option<Adc>,
    pwm: Option<Pwm>,
}

impl System {
    pub fn new() -> Self {
        Self {
            clock: SystemClock::default(),
            gpio: None,
            i2c: None,
            spi: None,
            uart: None,
            adc: None,
            pwm: None,
        }
    }

    pub fn init(&mut self, config: SystemConfig) -> Result<()> {
        if SYSTEM_READY.load(Ordering::SeqCst) {
            return Err(EmbedError::AlreadyInitialized);
        }

        // Initialize clock
        self.clock = config.clock.unwrap_or(SystemClock::default());

        // Initialize peripherals
        if config.enable_gpio {
            self.gpio = Some(Gpio::new()?);
        }
        if config.enable_i2c {
            self.i2c = Some(I2c::new(config.i2c_frequency_hz)?);
        }
        if config.enable_spi {
            self.spi = Some(Spi::new(config.spi_frequency_hz)?);
        }
        if config.enable_uart {
            self.uart = Some(Uart::new(config.uart_baud_rate)?);
        }
        if config.enable_adc {
            self.adc = Some(Adc::new()?);
        }
        if config.enable_pwm {
            self.pwm = Some(Pwm::new()?);
        }

        SYSTEM_READY.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn gpio(&self) -> Result<&Gpio> {
        self.gpio.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn i2c(&self) -> Result<&I2c> {
        self.i2c.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn spi(&self) -> Result<&Spi> {
        self.spi.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn uart(&self) -> Result<&Uart> {
        self.uart.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn adc(&self) -> Result<&Adc> {
        self.adc.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn pwm(&self) -> Result<&Pwm> {
        self.pwm.as_ref().ok_or(EmbedError::NotInitialized)
    }

    pub fn clock(&self) -> SystemClock {
        self.clock
    }

    pub fn is_ready() -> bool {
        SYSTEM_READY.load(Ordering::SeqCst)
    }
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SystemConfig {
    pub clock: Option<SystemClock>,
    pub enable_gpio: bool,
    pub enable_i2c: bool,
    pub i2c_frequency_hz: u32,
    pub enable_spi: bool,
    pub spi_frequency_hz: u32,
    pub enable_uart: bool,
    pub uart_baud_rate: u32,
    pub enable_adc: bool,
    pub enable_pwm: bool,
    pub watchdog: bool,
    pub watchdog_timeout_ms: u32,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            clock: None,
            enable_gpio: true,
            enable_i2c: true,
            i2c_frequency_hz: 100_000,
            enable_spi: true,
            spi_frequency_hz: 1_000_000,
            enable_uart: true,
            uart_baud_rate: 115200,
            enable_adc: true,
            enable_pwm: true,
            watchdog: false,
            watchdog_timeout_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMode {
    Active,
    Sleep,
    DeepSleep,
    Standby,
    Shutdown,
}

pub struct PowerManager {
    current_mode: PowerMode,
}

impl PowerManager {
    pub fn new() -> Self {
        Self {
            current_mode: PowerMode::Active,
        }
    }

    pub fn set_mode(&mut self, mode: PowerMode) -> Result<()> {
        // Hardware-specific power mode transition
        self.current_mode = mode;
        Ok(())
    }

    pub fn current_mode(&self) -> PowerMode {
        self.current_mode
    }

    pub fn enter_sleep(&mut self) -> Result<()> {
        self.set_mode(PowerMode::Sleep)
    }

    pub fn enter_deep_sleep(&mut self) -> Result<()> {
        self.set_mode(PowerMode::DeepSleep)
    }

    pub fn wakeup(&mut self) -> Result<()> {
        self.set_mode(PowerMode::Active)
    }
}

impl Default for PowerManager {
    fn default() -> Self {
        Self::new()
    }
}

// Global system instance (for no_std environments)
pub static mut SYSTEM: Option<System> = None;

pub fn system() -> Result<&'static mut System> {
    unsafe {
        SYSTEM.as_mut().ok_or(EmbedError::NotInitialized)
    }
}

pub fn init_system(config: SystemConfig) -> Result<&'static mut System> {
    unsafe {
        if SYSTEM.is_some() {
            return Err(EmbedError::AlreadyInitialized);
        }
        let mut sys = System::new();
        sys.init(config)?;
        SYSTEM = Some(sys);
        Ok(SYSTEM.as_mut().unwrap())
    }
}