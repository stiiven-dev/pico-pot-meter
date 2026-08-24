# pico-pot-meter

Two potentiometers driving an OLED bar-graph meter, with a calibration routine — no debug probe required.

<!-- TODO: hero photo of the breadboard -->
`docs/images/breadboard.jpg`

<!-- TODO: demo GIF — bars moving as pots turn, then the calibration routine -->
`docs/images/demo.gif`

---

## Features

- Two-pot bar-graph + numeric percentage on a 128×64 I²C OLED, ≥20 fps.
- Calibration routine: hold the button at both physical extremes of a pot to store its min/max, so the meter reads 0–100% across whatever range the pot actually swings (handles inverted wiring too).
- ADC oversampling + EMA filtering — see [`crates/pot-core`](crates/pot-core) for the pure, host-testable mapping/filter logic.
- Structured logging over USB-serial (`defmt-serial`) and crash capture (`panic-persist`), reused from `blinky-plus`.
- Fully USB-only dev loop, same as every project in this series.

## To-Do list

- [x] raw ADC read, logged over defmt
- [ ] filter comparison: raw vs EMA vs median (pick one, document why in README)
- [ ] OLED bring-up + bar graph rendering
- [ ] calibration routine (hold-both-extremes) + RAM persistence
- [ ] host tests for `raw_to_percent` (clamping, inverted pot)
- [ ] CI green
- [ ] `v0.1.0` release

## Hardware

| Part | Qty | Notes |
| --- | --- | --- |
| Raspberry Pi Pico W (or WH) | 1 | RP2040 + CYW43439 |
| Potentiometer (10 kΩ) | 2 | |
| SSD1306/SSD1309 0.96" 128×64 I²C OLED | 1 | |
| Breadboard + jumper wires | — | |
| USB micro-B cable | 1 | data-capable |

No debug probe needed — same USB-only workflow as `blinky-plus`.

## Wiring

| Pico W pin | Signal | Notes |
| --- | --- | --- |
| GP26 (pin 31) | Pot #1 wiper | ADC0 |
| GP27 (pin 32) | Pot #2 wiper | ADC1 |
| GP4 (pin 6) | OLED SDA | I2C0 |
| GP5 (pin 7) | OLED SCL | I2C0 |
| 3V3 (pin 36) | Pot ends + OLED VCC | |
| GND (pin 38) | Common ground | shared by both pots and OLED |

<!-- TODO: docs/wiring.md with the full diagram -->
No smoothing cap on the pot wipers in this build — filtering is done entirely in software (oversampling + EMA in `pot-core`); see that crate's docs for why.

## Quickstart

```bash
# one-time setup — skip if already done for blinky-plus
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs --locked
cargo install defmt-print --locked

cargo run --release -p firmware
```

Watch logs the same way as `blinky-plus`

```bash
./watch-defmt.sh target/thumbv6m-none-eabi/release/firmware
```

### Calibration

Hold the button while turning a pot to both of its physical extremes, release to store. Repeat per pot. Calibration lives in RAM only in this project (flash persistence is a later-project concept) — it resets on power-cycle, which is fine for a bring-up project and is called out explicitly rather than left as a surprise.

## Architecture

```text
pico-pot-meter/
├── crates/pot-core/     # no_std-compatible, zero HAL deps — pure mapping + filter logic
│   └── src/lib.rs       #   raw_to_percent(), Ema — both unit-tested on the host
└── firmware/            # thin adapter: ADC reads, OLED rendering, button handling
    └── src/main.rs
```

`pot-core` takes raw ADC counts + calibration bounds in, returns a clamped percentage out. It has no idea an RP2040 exists. That's deliberate — it's the piece you can actually step through with a real debugger on your laptop, which matters more here than usual since there's no SWD probe for the firmware side. `firmware/` stays thin: read ADC, call into `pot-core`, draw the result.

## Testing

```bash
cargo test -p pot-core   # host tests: clamping, inverted pot, filter convergence
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all -- --check
```

`firmware/` has no host tests — ADC timing and OLED rendering need real hardware to verify.

## Known limitations

- Calibration is RAM-only; power-cycling loses it. Flash-backed calibration is a later-project concept.
- No debug probe support — see [`blinky-plus`](../blinky-plus) for what that means in practice for this series.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
