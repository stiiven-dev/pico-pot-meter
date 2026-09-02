#![no_std]
#![no_main]

mod debouncer;
use debouncer::{ButtonEvent, ButtonMonitor};

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

use core::cell::RefCell;
use core::fmt::Write as _;
use cortex_m_rt::entry;
use critical_section::Mutex;
use static_cell::StaticCell;

use panic_persist as _;

use hal::{
    clocks::init_clocks_and_plls, gpio::Pins, pac, sio::Sio, timer::Timer, usb::UsbBus,
    watchdog::Watchdog,
};
use pot_core::{bar_fill_height, raw_to_percent, Calibration, Ema};
use rp2040_hal::fugit::RateExtU32;
use rp2040_hal::{self as hal, adc::AdcPin, rom_data, Adc};
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

type UsbState = (UsbDevice<'static, UsbBus>, SerialPort<'static, UsbBus>);
static USB_STATE: Mutex<RefCell<Option<UsbState>>> = Mutex::new(RefCell::new(None));

// --- Button-triggered behavior tuning (timer ticks = microseconds) ---
const DEBOUNCE_TICKS: u64 = 10_000;
const MULTI_CLICKS_WINDOW_TICKS: u64 = 400_000;
const HOLD_TICKS: u64 = 1_500_000;
// --- Other consts ---
const OVERSAMPLE_COUNT: u32 = 32;
const ALPHA: f32 = 0.2;
const PRINT_RATE: u64 = 500_000;
const POLL_BUTTON_TICKS: u64 = 5_000;
const DISPLAY_PERIOD_TICKS: u64 = 50_000; //instead of delay_ms(50)
const BAR_WIDTH: u32 = 20;
const BAR_MAX_HEIGHT: u32 = 30;
const BAR_Y_BASE: i32 = 40; // bottom of the bar area

struct DefmtUsbWriter;

impl embedded_io::ErrorType for DefmtUsbWriter {
    type Error = core::convert::Infallible;
}

impl embedded_io::Write for DefmtUsbWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let mut written = 0;
        let mut idle_polls = 0u32;
        const MAX_IDLE_POLLS: u32 = 200; // bound the spin — drop the rest if host isn't draining

        while written < buf.len() && idle_polls < MAX_IDLE_POLLS {
            critical_section::with(|cs| {
                if let Some((usb_dev, serial)) = USB_STATE.borrow_ref_mut(cs).as_mut() {
                    usb_dev.poll(&mut [serial]);
                    match serial.write(&buf[written..]) {
                        Ok(n) if n > 0 => {
                            written += n;
                            idle_polls = 0; // reset — we're making progress
                        }
                        Ok(_) | Err(UsbError::WouldBlock) => {
                            idle_polls += 1;
                        }
                        Err(_) => idle_polls = MAX_IDLE_POLLS, // bail on real errors
                    }
                }
            });
        }
        // Whether we wrote everything or gave up, tell the caller it's done —
        // dropping unsent log bytes is fine; hanging the app is not.
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn wait_for_usb_settle(timer: &Timer, settle_ticks: u64) {
    let start = timer.get_counter().ticks();
    while timer.get_counter().ticks() - start < settle_ticks {
        poll_usb();
    }
}

fn poll_usb() {
    critical_section::with(|cs| {
        if let Some((usb_dev, serial)) = USB_STATE.borrow_ref_mut(cs).as_mut() {
            usb_dev.poll(&mut [serial]);
        }
    });
}

fn draw_bars<D>(
    display: &mut D,
    pot1_pct: u8,
    pot2_pct: u8,
    calibrating: bool,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    display.clear(BinaryColor::Off)?;
    draw_one_bar(display, 10, pot1_pct)?;
    draw_one_bar(display, 70, pot2_pct)?;
    if calibrating {
        let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        Text::new("CAL", Point::new(40, 30), style).draw(display)?;
    }
    Ok(())
}

fn draw_one_bar<D>(display: &mut D, x: i32, pct: u8) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let fill_height = bar_fill_height(pct, BAR_MAX_HEIGHT);

    // outline, full height
    Rectangle::new(
        Point::new(x, BAR_Y_BASE - BAR_MAX_HEIGHT as i32),
        Size::new(BAR_WIDTH, BAR_MAX_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    // filled portion, grows upward from the base
    Rectangle::new(
        Point::new(x, BAR_Y_BASE - fill_height as i32),
        Size::new(BAR_WIDTH, fill_height),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;

    // percentage label underneath
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let mut buf = heapless::String::<8>::new();
    let _ = write!(buf, "{}%", pct);
    Text::new(&buf, Point::new(x, BAR_Y_BASE + 10), style).draw(display)?;

    Ok(())
}

static WRITER: StaticCell<DefmtUsbWriter> = StaticCell::new();
static USB_BUS: StaticCell<UsbBusAllocator<UsbBus>> = StaticCell::new();

/// Second-stage bootloader. Required: the RP2040 boot ROM reads this first
/// to know how to talk to QSPI flash. Without it, the chip can't validate
/// the flashed image and falls back to bootloader/mass-storage mode on
/// every reset instead of running the program.
#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_GENERIC_03H;

#[entry]
fn main() -> ! {
    //take ownership of rp2040 peripherals (singleton)
    let mut pac = pac::Peripherals::take().unwrap();

    //Watchdog
    let mut watchdog = Watchdog::new(pac.WATCHDOG);

    //Initialize clocks
    let clocks = init_clocks_and_plls(
        12_000_000,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    //SIO gives access to GPIO
    let sio = Sio::new(pac.SIO);

    //Initialize GPIO Pins
    let pins = Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );
    //Enable I2C peripheral
    let sda = pins
        .gpio4
        .reconfigure::<hal::gpio::FunctionI2C, hal::gpio::PullUp>();
    let scl = pins
        .gpio5
        .reconfigure::<hal::gpio::FunctionI2C, hal::gpio::PullUp>();
    let i2c = rp2040_hal::I2C::i2c0(
        pac.I2C0,
        sda,
        scl,
        400.kHz(),
        &mut pac.RESETS,
        &clocks.peripheral_clock,
    );

    //Enable ADC peripheral
    let mut adc = Adc::new(pac.ADC, &mut pac.RESETS);

    //enable Adc pin 0 and 1
    let mut adc_pin0 = AdcPin::new(pins.gpio26.into_floating_input()).unwrap();
    let mut adc_pin1 = AdcPin::new(pins.gpio27.into_floating_input()).unwrap();

    //GPIO12 as button (pin 16)
    let button_pin = pins.gpio12.into_pull_up_input();

    //Timer
    let timer = Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);
    //USB comms
    let usb_bus = USB_BUS.init(UsbBusAllocator::new(UsbBus::new(
        pac.USBCTRL_REGS,
        pac.USBCTRL_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    )));
    let serial = SerialPort::new(usb_bus);
    let usb_dev = UsbDeviceBuilder::new(usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .strings(&[StringDescriptors::new(LangID::EN).product("pico-pot-meter")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    critical_section::with(|cs| {
        *USB_STATE.borrow_ref_mut(cs) = Some((usb_dev, serial));
    });

    defmt_serial::defmt_serial(WRITER.init(DefmtUsbWriter));

    wait_for_usb_settle(&timer, 3_000_000);
    defmt::info!("pico-pot-meter is up!");

    if let Some(msg) = panic_persist::get_panic_message_utf8() {
        defmt::error!("previous boot panicked: {}", msg);
    }

    let now0 = timer.get_counter().ticks();
    let mut button = ButtonMonitor::new(
        button_pin,
        true,
        DEBOUNCE_TICKS,
        MULTI_CLICKS_WINDOW_TICKS,
        HOLD_TICKS,
        now0,
    )
    .unwrap();

    //EMA init
    let mut ema1 = Ema::new(ALPHA);
    let mut ema2 = Ema::new(ALPHA);
    //needed time for different events
    let mut info_last_time = 0u64;
    let mut button_last_time = 0u64;
    let mut display_last_time = 0u64;

    //Display init
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    //Calibration init
    let mut cal1 = Calibration::full_range();
    let mut cal2 = Calibration::full_range();
    let mut sweep1 = Calibration::start_sweep();
    let mut sweep2 = Calibration::start_sweep();
    let mut calibrating = false;

    loop {
        poll_usb();
        let now = timer.get_counter().ticks();
        if now.wrapping_sub(button_last_time) >= POLL_BUTTON_TICKS {
            button_last_time = now;
            match button.update(now).unwrap() {
                ButtonEvent::HoldTriggered => {
                    if calibrating {
                        if sweep1.is_valid() && sweep2.is_valid() {
                            cal1 = sweep1;
                            cal2 = sweep2;
                            defmt::info!(
                                "Calibration saved: pot1 {}...{}  pot2 {}...{}",
                                cal1.min,
                                cal1.max,
                                cal2.min,
                                cal2.max
                            );
                        } else {
                            defmt::warn!("Calibration incomplete - pot wasn't swept, keeping previous values");
                        }
                        calibrating = false;
                    } else {
                        sweep1 = Calibration::start_sweep();
                        sweep2 = Calibration::start_sweep();
                        calibrating = true;
                        defmt::info!("calibration started - sweep both pots, hold again to finish");
                    }
                }
                ButtonEvent::Clicks(n) if n >= 3 => {
                    defmt::warn!("button pressed more than 3 times — rebooting into flash");
                    // Give the USB writer a chance to flush the log line above
                    // before we reset (best-effort; no delay primitive wired
                    // in yet, so this is a very rough flush attempt).
                    poll_usb();
                    rom_data::reset_to_usb_boot(0, 0);
                }
                ButtonEvent::Clicks(n) => {
                    defmt::info!("clicked {} time(s)", n);
                }
                ButtonEvent::None => {}
            }
        }

        if now.wrapping_sub(display_last_time) >= DISPLAY_PERIOD_TICKS {
            //oversampling
            let mut pot1_sum: u32 = 0; //u32 because worst case scenario is 131_040 which surpasses u16::MAX
            let mut pot2_sum: u32 = 0;

            for _ in 0..OVERSAMPLE_COUNT {
                let raw_val1 = adc.read(&mut adc_pin0).unwrap();
                let raw_val2 = adc.read(&mut adc_pin1).unwrap();
                pot1_sum += raw_val1 as u32;
                pot2_sum += raw_val2 as u32;
            }
            let pot1_raw = (pot1_sum / OVERSAMPLE_COUNT) as u16;
            let pot2_raw = (pot2_sum / OVERSAMPLE_COUNT) as u16;

            //EMA
            let pot1_smoothed = ema1.update(pot1_raw as f32);
            let pot2_smoothed = ema2.update(pot2_raw as f32);

            if calibrating {
                sweep1.observe(pot1_smoothed as u16);
                sweep2.observe(pot2_smoothed as u16);
            }

            let pct1 = raw_to_percent(pot1_smoothed as u16, cal1.min, cal1.max);
            let pct2 = raw_to_percent(pot2_smoothed as u16, cal2.min, cal2.max);

            if now - info_last_time >= PRINT_RATE {
                info_last_time = now;
                defmt::info!(
                    "ema1={=u16} ema2={=u16}",
                    pot1_smoothed as u16,
                    pot2_smoothed as u16
                );
            }
            display_last_time = now;
            //drawing
            draw_bars(&mut display, pct1, pct2, calibrating).unwrap();
            display.flush().unwrap();
        }
    }
}
