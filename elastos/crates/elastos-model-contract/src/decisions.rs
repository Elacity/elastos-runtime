//! Bounded inline Choice decisions carried by the ordinary model run contract.

use crate::{ContractError, ContractResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const OPERATION: &str = "decision.evaluate";
pub const INPUT_SCHEMA: &str = "elastos.model.input.decisions/v1";
pub const OUTPUT_SCHEMA: &str = "elastos.model.output.decisions/v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub schema: String,
    pub state: Value,
    pub questions: BTreeMap<String, ChoiceQuestion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceQuestion {
    #[serde(rename = "type")]
    pub kind: String,
    pub instructions: String,
    pub criteria: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceAnswer {
    #[serde(rename = "type")]
    pub kind: String,
    pub choice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub schema: String,
    pub model: String,
    pub answers: BTreeMap<String, ChoiceAnswer>,
}

fn require(condition: bool, message: &str) -> ContractResult<()> {
    if condition {
        Ok(())
    } else {
        Err(ContractError::new(message))
    }
}

fn bounded(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.trim() == text && text.len() <= limit
}

fn probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

impl Input {
    pub fn validate(&self) -> ContractResult<()> {
        require(self.schema == INPUT_SCHEMA, "invalid decision input schema")?;
        require(
            self.state.is_string() || self.state.is_object() || self.state.is_array(),
            "invalid decision state",
        )?;
        require(
            (1..=8).contains(&self.questions.len()),
            "invalid decision question count",
        )?;
        for (id, question) in &self.questions {
            require(
                bounded(id, 64) && question.kind == "choice",
                "invalid decision question",
            )?;
            require(
                bounded(&question.instructions, 4096),
                "invalid decision instructions",
            )?;
            require(
                (2..=16).contains(&question.criteria.len()),
                "invalid decision choices",
            )?;
            for (choice, description) in &question.criteria {
                require(
                    bounded(choice, 64) && bounded(description, 4096),
                    "invalid decision criterion",
                )?;
            }
        }
        require(
            serde_json::to_vec(self)?.len() <= 32768,
            "decision input exceeds limit",
        )
    }
}

impl Output {
    pub fn validate(&self) -> ContractResult<()> {
        require(
            self.schema == OUTPUT_SCHEMA && bounded(&self.model, 256),
            "invalid decision output schema or model",
        )?;
        require(
            (1..=8).contains(&self.answers.len()),
            "invalid decision answer count",
        )?;
        for (id, answer) in &self.answers {
            require(
                bounded(id, 64) && answer.kind == "choice" && bounded(&answer.choice, 64),
                "invalid decision answer",
            )?;
            require(
                answer.confidence.is_none_or(probability),
                "invalid decision confidence",
            )?;
            if let Some(probabilities) = &answer.probabilities {
                require(
                    (2..=16).contains(&probabilities.len())
                        && probabilities.contains_key(&answer.choice),
                    "invalid decision probabilities",
                )?;
                require(
                    probabilities
                        .iter()
                        .all(|(key, value)| bounded(key, 64) && probability(*value)),
                    "invalid decision probability",
                )?;
                let selected = probabilities[&answer.choice];
                require(
                    probabilities
                        .values()
                        .all(|value| *value <= selected + 0.001),
                    "decision choice contradicts probabilities",
                )?;
                // Permit rounding in the upstream distribution, not missing mass.
                require(
                    (probabilities.values().sum::<f64>() - 1.0).abs() <= 0.001,
                    "decision probabilities do not sum to one",
                )?;
            }
        }
        Ok(())
    }

    pub fn validate_for(&self, input: &Input, model: &str) -> ContractResult<()> {
        self.validate()?;
        require(
            self.model == model,
            "decision model differs from selected model",
        )?;
        require(
            self.answers.keys().eq(input.questions.keys()),
            "decision answers differ from questions",
        )?;
        for (id, answer) in &self.answers {
            let criteria = &input.questions[id].criteria;
            require(
                criteria.contains_key(&answer.choice),
                "decision choice was not requested",
            )?;
            if let Some(probabilities) = &answer.probabilities {
                require(
                    probabilities.keys().eq(criteria.keys()),
                    "decision probabilities differ from choices",
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decisions_bind_model_question_and_choice_without_inventing_confidence() {
        let input: Input = serde_json::from_value(json!({"schema": INPUT_SCHEMA, "state": "A bounded test", "questions": {"review": {"type": "choice", "instructions": "Choose a review outcome", "criteria": {"allow": "Authorized", "defer": "Needs review"}}}})).unwrap();
        input.validate().unwrap();
        let mut output: Output = serde_json::from_value(json!({"schema": OUTPUT_SCHEMA, "model": "typesafe/jev-1.13", "answers": {"review": {"type": "choice", "choice": "defer"}}})).unwrap();
        output.validate_for(&input, "typesafe/jev-1.13").unwrap();
        assert!(output.answers["review"].confidence.is_none());
        assert!(output.validate_for(&input, "different-model").is_err());
        output.answers.get_mut("review").unwrap().choice = "invented".into();
        assert!(output.validate_for(&input, "typesafe/jev-1.13").is_err());
    }

    #[test]
    fn decisions_reject_invalid_distributions_and_unrequested_answers() {
        let mut output: Output = serde_json::from_value(json!({"schema": OUTPUT_SCHEMA, "model": "typesafe/jev-1.13", "answers": {"review": {"type": "choice", "choice": "defer", "confidence": 0.8, "probabilities": {"allow": 0.2, "defer": 0.8}}}})).unwrap();
        output.validate().unwrap();
        output.answers.get_mut("review").unwrap().confidence = Some(f64::NAN);
        assert!(output.validate().is_err());
        output.answers.get_mut("review").unwrap().confidence = None;
        output
            .answers
            .get_mut("review")
            .unwrap()
            .probabilities
            .as_mut()
            .unwrap()
            .insert("allow".into(), 0.7);
        assert!(output.validate().is_err());
    }
}
