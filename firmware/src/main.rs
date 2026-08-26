#![no_std]
#![no_main]

mod debouncer;
use debouncer::{ButtonEvent, ButtonMonitor};

use core::cell::RefCell;
use cortex_m_rt::entry;
use critical_section::Mutex;
//use embedded_hal::digital::OutputPin;
use static_cell::StaticCell;

use panic_persist as _;

use hal::{
    clocks::init_clocks_and_plls, gpio::Pins, pac, sio::Sio, timer::Timer, usb::UsbBus,
    watchdog::Watchdog,
};
use rp2040_hal::{self as hal, adc::AdcPin, rom_data, Adc};

use pot_core::/*raw_to_percent,*/ Ema;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

type UsbState = (UsbDevice<'static, UsbBus>, SerialPort<'static, UsbBus>);
static USB_STATE: Mutex<RefCell<Option<UsbState>>> = Mutex::new(RefCell::new(None));

// --- Button-triggered behavior tuning (timer ticks = microseconds) ---
const DEBOUNCE_TICKS: u64 = 20_000;
const MULTI_CLICKS_WINDOW_TICKS: u64 = 400_000;
const HOLD_TICKS: u64 = 1_500_000;
//Other consts
const OVERSAMPLE_COUNT: u32 = 32;
const ALPHA: f32 = 0.2;
//const PRINT_RATE: u64 = 500_000;
const MEDIAN_WINDOW: usize = 5;

struct DefmtUsbWriter;

impl embedded_io::ErrorType for DefmtUsbWriter {
    type Error = core::convert::Infallible;
}

impl embedded_io::Write for DefmtUsbWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let mut written = 0;
        while written < buf.len() {
            critical_section::with(|cs| {
                if let Some((usb_dev, serial)) = USB_STATE.borrow_ref_mut(cs).as_mut() {
                    usb_dev.poll(&mut [serial]);
                    match serial.write(&buf[written..]) {
                        Ok(n) => written += n,
                        Err(UsbError::WouldBlock) => {} // try again next loop
                        Err(_) => return, // real error, give up on this frame
                    }
                }
            });
        }
        Ok(written)
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

fn median_of(buf: &[u16; MEDIAN_WINDOW]) -> u16 {
    let mut sorted = *buf;
    sorted.sort_unstable();
    sorted[MEDIAN_WINDOW / 2]
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

    //Enable ADC peripheral
    let mut adc = Adc::new(pac.ADC, &mut pac.RESETS);

    //enable Adc pin 0 and 1
    let mut adc_pin0 = AdcPin::new(pins.gpio26.into_floating_input()).unwrap();
    //let mut adc_pin1 = AdcPin::new(pins.gpio27.into_floating_input()).unwrap();

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
        .strings(&[StringDescriptors::new(LangID::EN).product("blinky-plus")])
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
    //median filtering
    let mut median_buff: [u16; MEDIAN_WINDOW] = [0; MEDIAN_WINDOW];
    let mut median_idx = 0;

    //EMA init
    let mut ema1 = Ema::new(ALPHA);
    //will show info in slower rate so we can read
    //let mut last_time = 0u64;
    loop {
        poll_usb();
        let now = timer.get_counter().ticks();
        match button.update(now).unwrap() {
            ButtonEvent::HoldTriggered => {
                defmt::info!("should calibrate")
            }
            ButtonEvent::Clicks(n) if n >= 3 => {
                defmt::warn!("button held — rebooting into flash");
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
        //oversampling
        let mut pot1_sum: u32 = 0; //u32 because worst case scenario is 131_040 which surpasses u16::MAX

        for _ in 0..OVERSAMPLE_COUNT {
            let raw_val1 = adc.read(&mut adc_pin0).unwrap();
            pot1_sum += raw_val1 as u32;
        }
        let pot1_raw = (pot1_sum / OVERSAMPLE_COUNT) as u16;
        //moving median
        median_buff[median_idx] = pot1_raw;
        median_idx = (median_idx + 1) % MEDIAN_WINDOW;
        let pot1_median = median_of(&median_buff);

        //EMA
        let pot1_smoothed = ema1.update(pot1_raw as f32);
        //suppose min and max until calibration
        //let pct1 = raw_to_percent(pot1_smoothed as u16, 0, 4095);
        // let now = timer.get_counter().ticks();
        // if now - last_time >= PRINT_RATE {
        //     last_time = now;
        //
        // }
        //doing it asap for plotting purposes
        defmt::info!("PLOT raw={=u16} ema={=u16} median={=u16}", pot1_raw, pot1_smoothed as u16, pot1_median);
    }
}
