#![no_std]
#![no_main]
use cortex_m_rt::entry;
use rp2040_hal as hal;
use fugit::{self, RateExtU32};
use hal::{clocks, pac};
use hal::gpio::{FunctionPio0, Pin, PullNone};
use hal::pio::PIOExt;
use pio::pio_asm;  // For PIO assembly macro

// Ensure we halt the program on panic (if we don't mention this crate it won't
// be linked)
use panic_halt as _;

/// Bootloader (not needed if using a BSP like rp-pico)
#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_GENERIC_03H;

const XTAL_FREQ_HZ: u32 = 12_000_000u32; // 12.0 Mhz
const SYS_CLOCK_HZ: u32 = 61_440_000u32; // 61.44 MHz

// Sample rate and required timings for pll to hit 61.44MHz system freq that divides nicely for out i2s
const VCO_FREQ: u32 = 1536;
const POST_DIVIDER_1: u8 = 5;
const POST_DIVIDER_2: u8 = 5;

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    // despite the complaints from the linter, this must be mutable for other things in the hal.
    // Said things just dont happen in this file.
    let mut _watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let mut clocks = clocks::ClocksManager::new(pac.CLOCKS);

    let xosc = hal::xosc::setup_xosc_blocking(pac.XOSC, XTAL_FREQ_HZ.Hz())
        .map_err(clocks::InitError::XoscErr)
        .unwrap();

    pub const PLL_SYS_61P44MHZ: hal::pll::PLLConfig = hal::pll::PLLConfig {
        vco_freq: fugit::HertzU32::MHz(VCO_FREQ),
        refdiv: 1,
        post_div1: POST_DIVIDER_1,
        post_div2: POST_DIVIDER_2,
    };

    pub const PLL_USB_48MHZ: hal::pll::PLLConfig = hal::pll::PLLConfig {
        vco_freq: fugit::HertzU32::MHz(1440),
        refdiv: 1,
        post_div1: 6,
        post_div2: 5,
    };

    let pll_sys = hal::pll::setup_pll_blocking(
        pac.PLL_SYS,
        xosc.operating_frequency().into(),
        PLL_SYS_61P44MHZ,
        &mut clocks,
        &mut pac.RESETS,
    )
    .unwrap();

    let pll_usb = hal::pll::setup_pll_blocking(
        pac.PLL_USB,
        xosc.operating_frequency().into(),
        PLL_USB_48MHZ,
        &mut clocks,
        &mut pac.RESETS,
    )
    .unwrap();

    clocks.init_default(&xosc, &pll_sys, &pll_usb).unwrap();

    let sio = hal::Sio::new(pac.SIO);

    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    let i2s_program = pio::pio_asm!(
        ".side_set 2",
        ".wrap_target",

        // LEFT CHANNEL (LRCK = 0)
        "pull block       side 0b00",
        "set x, 31        side 0b00",

        // I2S delay bit (LRCK already 0 here)
        "nop              side 0b01",

        "left_loop:",
            "out pins 1       side 0b00",
            "nop              side 0b01",
            "jmp x-- left_loop side 0b01",

            // RIGHT CHANNEL (LRCK = 1)
            "pull block       side 0b10",
            "set x, 31        side 0b10",

            // I2S delay bit (LRCK transitions HERE)
            "nop              side 0b11",

        "right_loop:",
            "out pins 1       side 0b10",
            "nop              side 0b11",
            "jmp x-- right_loop side 0b11",

        ".wrap"
    );

    // Configure pins for PIO (matches Pimoroni Pico Audio: DATA=9, BCLK=10, LRCK=11)
    let _: Pin<_, FunctionPio0, PullNone> = pins.gpio9.reconfigure();
    let _: Pin<_, FunctionPio0, PullNone> = pins.gpio10.reconfigure();
    let _: Pin<_, FunctionPio0, PullNone> = pins.gpio11.reconfigure();
    let pin9_i2s_data: u8 = 9u8;
    let pin10_i2s_bclk: u8 = 10u8;
    let pin11_i2s_lrck: u8 = 11u8;

    // Split PIO0
    let (mut pio, sm0, _, _, _) = pac.PIO0.split(&mut pac.RESETS);
    
    // Install programs
    let program = pio.install(&i2s_program.program).unwrap();

    // Build SM
    let (mut sm, _, mut tx) = rp2040_hal::pio::PIOBuilder::from_installed_program(program)
        .out_pins(pin9_i2s_data, 1)
        .side_set_pin_base(pin10_i2s_bclk) // BCLK = base, LRCK = base+1
        .clock_divisor_fixed_point(10, 0) 
        .autopull(true)
        .pull_threshold(32)
        .build(sm0);

    // Set pin directions
    sm.set_pindirs([
        (pin9_i2s_data, hal::pio::PinDir::Output),
        (pin10_i2s_bclk, hal::pio::PinDir::Output),
        (pin11_i2s_lrck, hal::pio::PinDir::Output),
    ]);

    sm.start();
    for _ in 0..8 {
        tx.write(0xFFFFFFFF);
        tx.write(0x00000000);
    }

    loop {
        tx.write(0xFFFFFFFF);
        tx.write(0x00000000);
    }
}
