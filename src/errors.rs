use std::{
    fmt::Display,
    fs::write,
    hash::{DefaultHasher, Hash, Hasher},
};

use serde::{Deserialize, Serialize};
use thirtyfour::{By, WebDriver};
use thiserror::Error;

use crate::{GenResult, create_path, email, get_data};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Error, Default)]
pub enum SignInFailure {
    #[error("Er zijn te veel incorrecte inlogpogingen in een korte periode gedaan")]
    TooManyTries,
    #[error("Inloggegevens kloppen niet")]
    IncorrectCredentials,
    #[error("Webcomm heeft een storing")]
    WebcomDown,
    #[error("Onbekende fout: {0}")]
    Other(String),
    #[error("Onbekende fout")]
    #[default]
    Unknown,
}

#[derive(Debug, Error, PartialEq, Clone, Serialize, Deserialize, Default)]
pub enum FailureType {
    #[error("Webcom ical was niet in staat na meerdere pogingen diensten correct in te laden")]
    TriesExceeded,
    #[error("Webcom ical kan geen verbinding maken met de interne browser")]
    GeckoEngine,
    #[error("Webcom ical kon niet inloggen. Fout: {0}")]
    SignInFailed(SignInFailure),
    #[error("Webcom ical kon geen verbinding maken met de Webcomm site")]
    ConnectError,
    #[error("Een niet-specifieke fout is opgetreden: {0}")]
    Other(String),
    #[error("Ok")]
    #[default]
    OK,
}

pub trait OptionResult<T> {
    fn result(self) -> GenResult<T>;
    fn result_reason(self, reason: &str) -> GenResult<T>;
}

impl<T> OptionResult<T> for Option<T> {
    fn result(self) -> GenResult<T> {
        match self {
            Some(value) => Ok(value),
            None => Err("Option Unwrap".into()),
        }
    }
    fn result_reason(self, reason: &str) -> GenResult<T> {
        match self {
            Some(value) => Ok(value),
            None => Err(reason.into()),
        }
    }
}

pub async fn check_sign_in_error(driver: &WebDriver) -> GenResult<FailureType> {
    error!("Sign in failed");
    match driver.find(By::Id("ctl00_lblMessage")).await {
        Ok(element) => {
            let element_text = element.text().await?;
            let sign_in_error_type = get_sign_in_error_type(&element_text);
            info!("Found error banner: {:?}", &sign_in_error_type);
            Ok(FailureType::SignInFailed(sign_in_error_type))
        }
        Err(_) => Err("Geen fout banner gevonden".into()),
    }
}

// See if there is a text which indicated webcom is offline
pub fn check_if_webcom_unavailable(h3_text: Option<String>) -> bool {
    match h3_text {
        Some(text) => {
            if text == "De servertoepassing is niet beschikbaar.".to_owned() {
                return true;
            }
        }
        None => (),
    };
    false
}

fn get_sign_in_error_type(text: &str) -> SignInFailure {
    match text {
        "Uw aanmelding was niet succesvol. Voer a.u.b. het personeelsnummer of 'naam, voornaam' in" => {
            SignInFailure::IncorrectCredentials
        }
        "Te veel verkeerde aanmeldpogingen" => SignInFailure::TooManyTries,
        _ => SignInFailure::Other(text.to_string()),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct IncorrectCredentialsCount {
    pub retry_count: i32,
    pub error: Option<SignInFailure>,
    pub previous_password_hash: Option<u64>,
}

impl IncorrectCredentialsCount {
    pub fn load() -> IncorrectCredentialsCount {
        let path = create_path("sign_in_failure_count.json");
        || -> GenResult<IncorrectCredentialsCount> {
            let failure_count_json = std::fs::read_to_string(path)?;
            Ok(serde_json::from_str::<IncorrectCredentialsCount>(
                &failure_count_json,
            )?)
        }()
        .unwrap_or_default()
    }

    fn save(&self) -> GenResult<()> {
        let path = create_path("sign_in_failure_count.json");
        let failure_counter_serialised = serde_json::to_string(self)?;
        Ok(write(path, failure_counter_serialised.as_bytes())?)
    }

    fn get_password_hash() -> GenResult<u64> {
        let (user, _properties) = get_data();
        let current_password = user.password.clone();
        let mut hasher = DefaultHasher::new();
        current_password.hash(&mut hasher);
        Ok(hasher.finish())
    }

    // If returning None, continue execution
    pub fn sign_in_failed_check(&mut self) -> GenResult<Option<SignInFailure>> {
        let (_user, properties) = get_data();
        let resend_error_mail_count = properties.signin_fail_mail_reduce;
        let sign_in_attempt_reduce = properties.signin_fail_execution_reduce;
        let return_value: Option<SignInFailure>;
        if let Some(previous_password_hash) = self.previous_password_hash
            && let Ok(current_password_hash) = Self::get_password_hash()
            && previous_password_hash != current_password_hash
        {
            info!("Password hash has changed, resuming execution");
            return Ok(None);
        }
        self.retry_count += 1;
        // else check if retry counter == reduce_ammount, if not, stop running
        // If incorrect credentials. Never execute unless the password has has changes
        return_value = match self.error.as_ref() {
            Some(SignInFailure::IncorrectCredentials) => {
                info!("Permanently Skipping execution due to incorrect credentials");
                self.error.clone()
            }
            _ => {
                if self.retry_count % sign_in_attempt_reduce == 0 {
                    warn!(
                        "Continuing execution with sign in error, reduce val: {sign_in_attempt_reduce}, current count {}",
                        self.retry_count
                    );
                    self.retry_count -= 1;
                    None
                } else {
                    self.error.clone()
                }
            }
        };

        if self.retry_count % resend_error_mail_count == 0 && self.error.is_some() {
            email::send_failed_signin_mail(&self, false)?;
        }
        self.save()?;
        Ok(return_value)
    }

    pub fn update_signin_failure(
        &mut self,
        failed: bool,
        failure_type: Option<SignInFailure>,
    ) -> GenResult<()> {
        if let Ok(current_password_hash) = Self::get_password_hash() {
            debug!("Got current password hash: {current_password_hash}");
            self.previous_password_hash = Some(current_password_hash);
        }
        // if failed == true, set increment counter and set error
        if failed {
            self.error = failure_type;
            // Send email about failed sign in if this is the first time it has happened
            if self.retry_count == 0 {
                self.retry_count += 1;
                email::send_failed_signin_mail(&self, true)?;
            }
        } else {
            // if failed == false, reset counter
            if self.error.is_some() {
                info!("Sign in succesful again!");
                email::send_sign_in_succesful()?;
            }
            self.retry_count = 0;
            self.error = None;
        }
        self.save()?;
        Ok(())
    }
}

pub trait ResultLog<T, E> {
    fn error(&self, function_name: &str);
    fn warn(&self, function_name: &str);
    fn warn_owned(self, function_name: &str) -> Self;
    fn info(&self, function_name: &str);
}

impl<T, E> ResultLog<T, E> for Result<T, E>
where
    E: Display,
{
    fn info(&self, function_name: &str) {
        match self {
            Err(err) => {
                info!("Error in function \"{function_name}\": {}", err.to_string())
            }
            _ => (),
        }
    }
    fn warn_owned(self, function_name: &str) -> Self {
        self.inspect_err(|err| warn!("Error in function \"{function_name}\": {}", err.to_string()))
    }
    fn warn(&self, function_name: &str) {
        match self {
            Err(err) => {
                warn!("Error in function \"{function_name}\": {}", err.to_string())
            }
            _ => (),
        }
    }
    fn error(&self, function_name: &str) {
        match self {
            Err(err) => {
                error!("Error in function \"{function_name}\": {}", err.to_string())
            }
            _ => (),
        }
    }
}
