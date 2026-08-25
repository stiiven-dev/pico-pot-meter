use embedded_hal::digital::InputPin;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    None,
    Pressed,
    Released,
}

pub enum ButtonEvent {
    None,
    Clicks(u8), //a click sequence of n has been fired
    HoldTriggered,
}

pub struct Debouncer<P> {
    pin: P,
    //physically pressed or not
    stable_pressed: bool,
    //raw reading currently being checked
    candidate_pressed: bool,
    //timer tick at which the candidate reading last changed
    candidate_since: u64,
    //threshold
    debounce_ticks: u64,

    active_low: bool,
}

impl<P, E> Debouncer<P>
where
    P: InputPin<Error = E>,
{
    // Create a debouncer, reading the pin once to seed the initial stable
    // state (avoids reporting a spurious edge on the very first `update`).
    //
    // `now` should be the current timer tick count at construction time.
    pub fn new(pin: P, active_low: bool, debounce_ticks: u64, now: u64) -> Result<Self, E> {
        let mut this = Self {
            pin,
            stable_pressed: false,
            candidate_pressed: false,
            candidate_since: now,
            debounce_ticks,
            active_low,
        };
        let pressed = this.read_pressed()?;
        this.stable_pressed = pressed;
        this.candidate_pressed = pressed;
        Ok(this)
    }

    fn read_pressed(&mut self) -> Result<bool, E> {
        let level_high = self.pin.is_high()?;
        Ok(if self.active_low {
            !level_high
        } else {
            level_high
        })
    }

    pub fn update(&mut self, now: u64) -> Result<Edge, E> {
        let raw_pressed = self.read_pressed()?;

        if raw_pressed != self.candidate_pressed {
            //raw reading changed
            self.candidate_pressed = raw_pressed;
            self.candidate_since = now;
            return Ok(Edge::None);
        }

        if self.candidate_pressed != self.stable_pressed
            && now.wrapping_sub(self.candidate_since) >= self.debounce_ticks
        {
            //candidate for enough time to be counted as valid
            self.stable_pressed = self.candidate_pressed;
            return Ok(if self.stable_pressed {
                Edge::Pressed
            } else {
                Edge::Released
            });
        }
        Ok(Edge::None)
    }

    pub fn is_pressed(&self) -> bool {
        self.stable_pressed
    }
}

//wraps a Debouncer to detect click sequences
pub struct ButtonMonitor<P> {
    debouncer: Debouncer<P>,
    click_count: u8,
    press_started: Option<u64>,
    last_release: Option<u64>,
    hold_fired: bool,
    // Max ticks between a release and the next press for it to count as
    // part of the same click sequence.
    multi_click_window_ticks: u64,
    //ticks of continuous press for a HoldTriggered fires
    hold_ticks: u64,
}

impl<P, E> ButtonMonitor<P>
where
    P: InputPin<Error = E>,
{
    pub fn new(
        pin: P,
        active_low: bool,
        debounce_ticks: u64,
        multi_click_window_ticks: u64,
        hold_ticks: u64,
        now: u64,
    ) -> Result<Self, E> {
        Ok(Self {
            debouncer: Debouncer::new(pin, active_low, debounce_ticks, now)?,
            click_count: 0,
            press_started: None,
            last_release: None,
            hold_fired: false,
            multi_click_window_ticks,
            hold_ticks,
        })
    }

    pub fn update(&mut self, now: u64) -> Result<ButtonEvent, E> {
        match self.debouncer.update(now)? {
            Edge::Pressed => {
                self.press_started = Some(now);
                self.hold_fired = false;
                let within_window = self
                    .last_release
                    .map(|t| now.wrapping_sub(t) <= self.multi_click_window_ticks)
                    .unwrap_or(false);
                self.click_count = if within_window {
                    self.click_count + 1
                } else {
                    1
                };
            }
            Edge::Released => {
                self.last_release = Some(now);
                self.press_started = None;
            }
            Edge::None => {}
        }
        if let Some(start) = self.press_started {
            if !self.hold_fired && now.wrapping_sub(start) >= self.hold_ticks {
                self.hold_fired = true;

                //prevent firing multiple clicks
                self.click_count = 0;
                return Ok(ButtonEvent::HoldTriggered);
            }
        }

        if self.click_count > 0 && !self.debouncer.is_pressed() {
            if let Some(last_release) = self.last_release {
                if now.wrapping_sub(last_release) > self.multi_click_window_ticks {
                    let n = self.click_count;
                    self.click_count = 0;
                    return Ok(ButtonEvent::Clicks(n));
                }
            }
        }
        Ok(ButtonEvent::None)
    }
}
