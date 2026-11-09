//! `/budget <usd>`: a cap on what the session costs. Before each request, minerve checks the
//! session's cost (the one in the status bar) against it, and pauses the turn - and a `/goal`
//! loop - once it is reached. Raise it, or turn it off, and send a message to go on.
//!
//! Only providers that report a cost count (OpenRouter does; Anthropic and OpenAI do not).

use std::sync::Mutex;

/// What `/budget <arg>` asks for.
#[derive(Debug, PartialEq)]
pub enum BudgetCommand {
    /// `/budget`: say what it is.
    Show,
    /// `/budget 2` or `/budget $2.50`.
    Set(f64),
    /// `/budget off`.
    Off,
}

pub fn parse(arg: &str) -> Result<BudgetCommand, String> {
    let arg = arg.trim();
    match arg.to_lowercase().as_str() {
        "" => return Ok(BudgetCommand::Show),
        "off" | "none" | "clear" | "0" => return Ok(BudgetCommand::Off),
        _ => {}
    }
    match arg.trim_start_matches('$').trim().parse::<f64>() {
        Ok(usd) if usd.is_finite() && usd > 0.0 => Ok(BudgetCommand::Set(usd)),
        _ => Err(format!("Usage: /budget <usd>, e.g. /budget 2, or /budget off. `{arg}` is not an amount.")),
    }
}

/// The cap, in USD. None when there is none.
#[derive(Default)]
pub struct Budget(Mutex<Option<f64>>);

impl Budget {
    pub fn get(&self) -> Option<f64> {
        *self.0.lock().unwrap()
    }

    /// Runs `/budget <arg>` and says what it did, given what the session cost so far and whether
    /// the active model's provider reports a cost.
    pub fn command(&self, arg: &str, spent: f64, provider_reports_cost: bool) -> String {
        let command = match parse(arg) {
            Ok(command) => command,
            Err(usage) => return usage,
        };
        let notice = match command {
            BudgetCommand::Show => match self.get() {
                Some(usd) => format!("Budget: ${usd:.2}, spent ${spent:.4}. /budget <usd> to change it, /budget off to drop it."),
                None => format!("No budget, spent ${spent:.4}. /budget <usd> pauses minerve once the session costs that much."),
            },
            BudgetCommand::Off => {
                *self.0.lock().unwrap() = None;
                return String::from("Budget off.");
            }
            BudgetCommand::Set(usd) => {
                *self.0.lock().unwrap() = Some(usd);
                let mut notice = format!("Budget: ${usd:.2}. Minerve pauses once the session costs that much (spent ${spent:.4}).");
                if spent >= usd {
                    notice.push_str(" Already reached: the next request pauses.");
                }
                notice
            }
        };
        match provider_reports_cost {
            true => notice,
            false => format!("{notice} This model's provider does not report a cost, so its requests do not count."),
        }
    }

    /// The notice to pause with, when `spent` has reached the cap.
    pub fn exceeded(&self, spent: f64) -> Option<String> {
        let usd = self.get()?;
        (spent >= usd).then(|| {
            format!("Paused: the session cost ${spent:.4}, over the ${usd:.2} budget. /budget <usd> to raise it (or /budget off), then send a message to go on.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_and_off_parse() {
        assert_eq!(parse("2"), Ok(BudgetCommand::Set(2.0)));
        assert_eq!(parse(" $2.50 "), Ok(BudgetCommand::Set(2.5)));
        assert_eq!(parse("off"), Ok(BudgetCommand::Off));
        assert_eq!(parse(""), Ok(BudgetCommand::Show));
        assert!(parse("lots").is_err());
        assert!(parse("-1").is_err());
    }

    #[test]
    fn a_budget_pauses_once_reached() {
        let budget = Budget::default();
        assert_eq!(budget.exceeded(100.0), None, "no budget, no pause");
        budget.command("2", 0.5, true);
        assert_eq!(budget.exceeded(1.99), None);
        assert!(budget.exceeded(2.0).is_some());
        budget.command("off", 2.0, true);
        assert_eq!(budget.exceeded(5.0), None);
    }

    #[test]
    fn a_provider_without_cost_is_said() {
        let notice = Budget::default().command("2", 0.0, false);
        assert!(notice.contains("does not report a cost"), "{notice}");
    }
}
