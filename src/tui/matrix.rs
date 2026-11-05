//! `/matrix`: falling letters over the whole screen.

use super::*;

/// Characters the falling streams are made of (same set as the Emacs `matrix-mode` it follows).
pub(super) const MATRIX_GLYPHS: &str = "123456789abcdefghijklmnopqrstuvwxyzαβγδεζηθικλμνξοπρστυφχψωΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩ";


/// Xorshift64 step, returning a float in [0, 1). Good enough for falling letters; avoids a crate.
pub(super) fn next_unit(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 40) as f32 / (1u64 << 24) as f32
}

/// One falling column of letters.
#[derive(Clone, Copy)]
pub(super) struct MatrixStream {
    /// Row of the leading (brightest) letter. Negative while the stream is still above the screen.
    head: f32,
    /// Rows per second.
    speed: f32,
    /// Trail length in rows, head included.
    length: u16,
}

impl MatrixStream {
    /// A new stream. `scattered` streams start somewhere on screen; the others start above it,
    /// so the screen fills in gradually.
    pub(super) fn random(rng: &mut u64, height: u16, scattered: bool) -> Self {
        let height = height.max(1) as f32;
        let head = if scattered {
            next_unit(rng) * height
        } else {
            -next_unit(rng) * height
        };
        Self {
            head,
            speed: 8.0 + next_unit(rng) * 20.0,
            length: 4 + (next_unit(rng) * height * 0.6) as u16,
        }
    }
}

/// Length of the fade-in when `/matrix` starts and the fade-out when it is stopped.
pub(super) const MATRIX_FADE: Duration = Duration::from_millis(500);

/// Where the `/matrix` screen is in its life cycle.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum MatrixPhase {
    /// Letters start falling from the top edge while the screen brightens.
    FadingIn,
    /// Full rain.
    Running,
    /// No new streams spawn; the existing ones fall off the bottom while the screen dims.
    FadingOut,
}

/// State of the `/matrix` screen: one stream per column, advanced in real time.
pub(super) struct MatrixRain {
    width: u16,
    height: u16,
    streams: Vec<MatrixStream>,
    glyphs: Vec<char>,
    rng: u64,
    last_tick: Instant,
    phase: MatrixPhase,
    phase_started: Instant,
    /// Brightness when `stop` was called, so a stop during the fade-in continues from there.
    stop_level: f32,
}

impl MatrixRain {
    pub(super) fn new(width: u16, height: u16) -> Self {
        let rng = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        let mut rain = Self {
            width: 0,
            height: 0,
            streams: Vec::new(),
            glyphs: MATRIX_GLYPHS.chars().collect(),
            rng,
            last_tick: Instant::now(),
            phase: MatrixPhase::FadingIn,
            phase_started: Instant::now(),
            stop_level: 1.0,
        };
        rain.resize(width, height);
        rain
    }

    /// Rebuilds the streams for a new size. Every stream starts above the top edge, so the
    /// letters fall in from the top.
    pub(super) fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.streams = (0..width)
            .map(|_| MatrixStream::random(&mut self.rng, height, false))
            .collect();
    }

    /// Starts the fade-out. Ignored once it has begun.
    pub(super) fn stop(&mut self) {
        if self.phase != MatrixPhase::FadingOut {
            self.stop_level = self.level();
            self.phase = MatrixPhase::FadingOut;
            self.phase_started = Instant::now();
        }
    }

    /// Brightness in [0, 1] that scales every colour.
    pub(super) fn level(&self) -> f32 {
        let t = (self.phase_started.elapsed().as_secs_f32() / MATRIX_FADE.as_secs_f32()).min(1.0);
        match self.phase {
            MatrixPhase::FadingIn => t,
            MatrixPhase::Running => 1.0,
            MatrixPhase::FadingOut => self.stop_level * (1.0 - t),
        }
    }

    /// Moves the phase on when its fade has finished.
    pub(super) fn update_phase(&mut self) {
        if self.phase == MatrixPhase::FadingIn && self.phase_started.elapsed() >= MATRIX_FADE {
            self.phase = MatrixPhase::Running;
        }
    }

    /// Moves every stream down by the time since the last draw. A stream that has left the
    /// bottom starts again above the top, unless the screen is fading out.
    pub(super) fn advance(&mut self) {
        let now = Instant::now();
        // Capped so a long pause (e.g. a suspended terminal) does not jump the streams.
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.2);
        self.last_tick = now;
        let height = self.height as f32;
        let stopping = self.phase == MatrixPhase::FadingOut;
        for stream in &mut self.streams {
            stream.head += stream.speed * dt;
            if stream.head - stream.length as f32 > height {
                if stopping {
                    // Length 0 draws nothing, so the stream is gone for good.
                    stream.length = 0;
                } else {
                    *stream = MatrixStream::random(&mut self.rng, self.height, false);
                }
            }
        }
    }

    /// Paints the rain over `area`. Returns false once the fade-out is over and the normal
    /// UI should come back.
    pub(super) fn draw(&mut self, frame: &mut Frame, area: Rect) -> bool {
        self.update_phase();
        if self.phase == MatrixPhase::FadingOut && self.level() <= 0.0 {
            return false;
        }
        if self.width != area.width || self.height != area.height {
            self.resize(area.width, area.height);
        }
        self.advance();

        let level = self.level();
        let scale = |value: f32| (value * level) as u8;
        let buf = frame.buffer_mut();
        for (col, stream) in self.streams.iter().enumerate() {
            let x = area.x + col as u16;
            for row in 0..area.height {
                // Rows above the leading letter; the trail is the first `length` of them.
                let distance = stream.head.floor() - row as f32;
                let lit = distance >= 0.0 && distance < stream.length as f32;
                let cell = &mut buf[(x, area.y + row)];
                cell.set_bg(Color::Black);
                if !lit {
                    cell.set_char(' ');
                    continue;
                }
                // Letters flicker: each drawn cell picks a new glyph.
                let index = (next_unit(&mut self.rng) * self.glyphs.len() as f32) as usize;
                let color = if distance < 1.0 {
                    let white = scale(255.0);
                    Color::Rgb(white, white, white)
                } else {
                    let fade = 1.0 - distance / stream.length as f32;
                    Color::Rgb(0, scale(60.0 + 195.0 * fade), 0)
                };
                cell.set_char(self.glyphs[index % self.glyphs.len()]).set_fg(color);
            }
        }
        true
    }
}
