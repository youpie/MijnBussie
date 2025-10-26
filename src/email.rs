use crate::errors::IncorrectCredentialsCount;
use crate::{GenError, GenResult, ShiftState, get_data};
use lettre::{
    Message, SmtpTransport, Transport, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use std::{collections::HashMap, fs, path::PathBuf};
use strfmt::strfmt;
use thirtyfour::error::{WebDriverErrorInfo, WebDriverResult};
use time::{Date, macros::format_description};
use url::Url;

use crate::{Shift, SignInFailure, create_ical_filename, create_shift_link, get_set_name};

const ERROR_VALUE: &str = "HIER HOORT WAT ANDERS DAN DEZE TEKST TE STAAN, CONFIGURATIE INCORRECT";
const SENDER_NAME: &str = "Peter";
pub const TIME_DESCRIPTION: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[hour]:[minute]");
pub const DATE_DESCRIPTION: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[day]-[month]-[year]");

const COLOR_BLUE: &str = "#1a5fb4";
const COLOR_RED: &str = "#a51d2d";
const COLOR_GREEN: &str = "#26a269";

trait StrikethroughString {
    fn strikethrough(&self) -> String;
}

impl StrikethroughString for String {
    fn strikethrough(&self) -> String {
        self.chars()
            .map(|c| format!("{}{}", c, '\u{0336}'))
            .collect()
    }
}

pub struct EnvMailVariables {
    pub smtp_server: String,
    pub smtp_username: String,
    pub smtp_password: String,
    pub mail_from: String,
    pub mail_to: String,
    mail_error_to: String,
    send_email_new_shift: bool,
    send_mail_updated_shift: bool,
    send_welcome_mail: bool,
    send_failed_signin_mail: bool,
    send_error_mail: bool,
    send_removed_shift: bool,
}

/*
Loads all env variables needed for sending mails
Does not load defaults if they are not found and will just error
If kuma is true, it adds KUMA_ to the var names to find ones specific for KUMA
*/
impl EnvMailVariables {
    pub fn new() -> Self {
        let (user, properties) = get_data();
        let email_properties = properties.general_email_properties.clone();
        let smtp_server = email_properties.smtp_server;
        let smtp_username = email_properties.smtp_username;
        let smtp_password = email_properties.smtp_password;
        let mail_from = email_properties.mail_from;
        let mail_to = user.email.clone();
        let mail_error_to = properties.support_mail.clone();
        let send_email_new_shift = user.user_properties.send_mail_new_shift;
        let send_mail_updated_shift = user.user_properties.send_mail_updated_shift;
        let send_error_mail = user.user_properties.send_error_mail;
        let send_welcome_mail = user.user_properties.send_welcome_mail;
        let send_removed_shift = user.user_properties.send_mail_removed_shift;
        let send_failed_signin_mail = user.user_properties.send_failed_signin_mail;
        Self {
            smtp_server,
            smtp_username,
            smtp_password,
            mail_from,
            mail_to,
            mail_error_to,
            send_email_new_shift,
            send_mail_updated_shift,
            send_error_mail,
            send_welcome_mail,
            send_failed_signin_mail,
            send_removed_shift,
        }
    }
}

/*
Main function for sending mails, it will always be called and will individually check if that function needs to be called
If loading previous shifts fails for whatever it will not error but just do an early return.
Because if the previous shifts file is not, it will just not send mails that time
Returns the list of previously known shifts, updated with new shits
*/
pub fn send_emails(
    current_shifts: Vec<Shift>,
    previous_shifts: Vec<Shift>,
) -> GenResult<Vec<Shift>> {
    let env = EnvMailVariables::new();
    let mailer = load_mailer(&env)?;
    if previous_shifts.is_empty() {
        // if the previous were empty, just return the list of current shifts as all new
        error!("!!! PREVIOUS SHIFTS WAS EMPTY. SKIPPING !!!");
        return Ok(current_shifts
            .into_iter()
            .map(|mut shift| {
                shift.state = ShiftState::New;
                shift
            })
            .collect());
    }
    Ok(find_send_shift_mails(
        &mailer,
        previous_shifts,
        current_shifts,
        &env,
    )?)
}

// Creates SMTPtransport from username, password and server found in env
fn load_mailer(env: &EnvMailVariables) -> GenResult<SmtpTransport> {
    let creds = Credentials::new(env.smtp_username.clone(), env.smtp_password.clone());
    let mailer = SmtpTransport::relay(&env.smtp_server)?
        .credentials(creds)
        .build();
    Ok(mailer)
}

/*
Will search for new shifts given previous shifts.
Will be ran twice, If provided new shifts, it will look for updated shifts instead
Will send an email is send_mail is true
It doesn't make a lot of sense that this function is in Email
*/
fn find_send_shift_mails(
    mailer: &SmtpTransport,
    previous_shifts: Vec<Shift>,
    new_shifts: Vec<Shift>,
    env: &EnvMailVariables,
) -> GenResult<Vec<Shift>> {
    let current_date: Date = Date::parse(
        &chrono::offset::Local::now().format("%d-%m-%Y").to_string(),
        DATE_DESCRIPTION,
    )?;
    let mut previous_shifts_map = previous_shifts
        .into_iter()
        .map(|shift| (shift.magic_number, shift))
        .collect::<HashMap<i64, Shift>>();
    // Iterate through the current shifts to check for updates or new shifts
    // We start with a list of previously valid shifts. All marked as deleted
    // we will then loop over a list of newly loaded shifts from the website
    for mut new_shift in new_shifts {
        // If the hash of this current shift is found in the previously valid shift list,
        // we know this shift has remained unchanged. So mark it as such
        if let Some(previous_shift) = previous_shifts_map.get_mut(&new_shift.magic_number) {
            previous_shift.state = ShiftState::Unchanged;
        } else {
            // if it is not found, we loop over the list of previously known shifts
            for previous_shift in previous_shifts_map.clone() {
                // if during the loop, we find a previously valid shift with the same starting date as the current shift
                // whereby we assume only 1 shift can be active per day
                // we know it must have changed, as if it hadn't it would have been found from its hash
                // so it can be marked as changed
                // We must first remove the old shift, then add the new shift
                if previous_shift.1.date == new_shift.date {
                    match previous_shifts_map.remove(&previous_shift.0) {
                        Some(_) => (),
                        None => warn!(
                            "Tried to remove shift {} as it has been updated, but that failed",
                            previous_shift.1.number
                        ),
                    };
                    new_shift.state = ShiftState::Changed;
                    previous_shifts_map.insert(new_shift.magic_number, new_shift.clone());
                    break;
                }
            }
            // If after that loop, no previously known shift with the same start date as the new shift was found
            // we know it is a new shift, so we mark it as such and add it to the list of known shifts
            if new_shift.state != ShiftState::Changed {
                new_shift.state = ShiftState::New;
                previous_shifts_map.insert(new_shift.magic_number, new_shift);
            }
            // Because we only loop over new shifts, all old and deleted shifts do not even get looked at. And since they start as deleted
            // They will be deleted
        }
    }
    let current_shift_vec: Vec<Shift> = previous_shifts_map.into_values().collect();
    let mut new_shifts: Vec<&Shift> = current_shift_vec
        .iter()
        .filter(|item| item.state == ShiftState::New)
        .collect();
    let mut updated_shifts: Vec<&Shift> = current_shift_vec
        .iter()
        .filter(|item| item.state == ShiftState::Changed)
        .collect();
    let mut removed_shifts: Vec<&Shift> = current_shift_vec
        .iter()
        .filter(|item| item.state == ShiftState::Deleted)
        .collect();
    // debug!("shift vec : {:#?}",current_shift_vec);
    debug!("Removed shift vec size: {}", removed_shifts.len());
    new_shifts.retain(|shift| shift.date >= current_date);
    if !new_shifts.is_empty() && env.send_email_new_shift {
        info!("Found {} new shifts, sending email", new_shifts.len());
        create_send_new_email(mailer, new_shifts, env, false)?;
    }
    updated_shifts.retain(|shift| shift.date >= current_date);
    if !updated_shifts.is_empty() && env.send_mail_updated_shift {
        info!(
            "Found {} updated shifts, sending email",
            updated_shifts.len()
        );
        create_send_new_email(mailer, updated_shifts, env, true)?;
    }
    if !removed_shifts.is_empty() && env.send_removed_shift {
        info!("Removing {} shifts", removed_shifts.len());
        removed_shifts.retain(|shift| shift.date >= current_date);
        if !removed_shifts.is_empty() {
            send_removed_shifts_mail(mailer, env, removed_shifts)?;
        }
    }
    // At last remove all shifts marked as removed from the vec
    let current_shift_vec = current_shift_vec
        .into_iter()
        .filter(|shift| shift.state != ShiftState::Deleted)
        .collect();
    Ok(current_shift_vec)
}

/*
Composes and sends mail with either new shifts or updated shifts if required. in plaintext
Depending on if update is true or false
Will always send under the name of Peter
*/
fn create_send_new_email(
    mailer: &SmtpTransport,
    new_shifts: Vec<&Shift>,
    env: &EnvMailVariables,
    update: bool,
) -> GenResult<()> {
    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let mut changed_mail_html = fs::read_to_string("./templates/changed_shift.html").unwrap();
    let shift_table = fs::read_to_string("./templates/shift_table.html").unwrap();
    let enkel_meervoud = if new_shifts.len() != 1 { "en" } else { "" };
    let name = get_set_name(None);
    let new_update_text = match update {
        true => "geupdate",
        false => "nieuwe",
    };

    let mut shift_tables = String::new();
    for shift in &new_shifts {
        let shift_table_clone = strfmt!(&shift_table,
            shift_number => shift.number.clone(),
            shift_date => shift.date.format(DATE_DESCRIPTION)?.to_string(),
            shift_start => shift.start.format(TIME_DESCRIPTION)?.to_string(),
            shift_end => shift.end.format(TIME_DESCRIPTION)?.to_string(),
            shift_duration_hour => shift.duration.whole_hours().to_string(),
            shift_duration_minute => (shift.duration.whole_minutes() % 60).to_string(),
            shift_link => create_shift_link(shift, false).unwrap_or_default(),
            bussie_login => if let Ok(url) = create_footer(true) {format!("/loginlink/{url}")} else {String::new()},
            shift_link_pdf => create_shift_link(shift, true).unwrap_or_default()
        )?;
        shift_tables.push_str(&shift_table_clone);
    }
    changed_mail_html = strfmt!(
        &changed_mail_html,
        name => name.clone(),
        shift_changed_ammount => new_shifts.len().to_string(),
        new_update => new_update_text.to_string(),
        single_plural => enkel_meervoud.to_string(),
        shift_tables => shift_tables.to_string()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => changed_mail_html,
        banner_color => COLOR_BLUE,
        footer => create_footer(false).unwrap_or(ERROR_VALUE.to_owned())
    )?;

    let email = Message::builder()
        .from(format!("Peter <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to).parse()?)
        .subject(format!(
            "Je hebt {} {} dienst{}",
            &new_shifts.len(),
            new_update_text,
            enkel_meervoud
        ))
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

fn create_footer(only_url: bool) -> GenResult<String> {
    let (_user, properties) = get_data();
    let footer_text = r#"<tr>
      <td style="background-color:#FFFFFF; text-align:center; padding-top:0px;font-size:12px;">
        <a style="color:#9a9996;">{footer_text}
      </td>
      <tr>
      <td style="background-color:#FFFFFF; text-align:center;font-size:12px;padding-bottom:10px;">
        <a href="{footer_url}" style="color:#9a9996;">{footer_url}</a>
      </td>
      <tr>
      <td style="background-color:#FFFFFF; text-align:center;font-size:12px;padding-bottom:10px;">
        <a style="color:#9a9996;">{admin_email_comment}</a>
      </td>
      </tr>"#;
    let domain = &properties.ical_domain;
    let url = Url::parse(domain)?;
    let url = url.join(&create_ical_filename())?;
    let admin_email = &properties.support_mail;
    let return_value = match only_url {
        true => url.to_string(),
        false => strfmt!(footer_text,
            footer_text => "Je agenda link:",
            footer_url => url.to_string(),
            admin_email_comment => format!("Vragen of opmerkingen? Neem contact op met {admin_email}"))
        .unwrap_or_default(),
    };
    Ok(return_value)
}

fn send_removed_shifts_mail(
    mailer: &SmtpTransport,
    env: &EnvMailVariables,
    removed_shifts: Vec<&Shift>,
) -> GenResult<()> {
    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let removed_shift_html = fs::read_to_string("./templates/removed_shift_base.html").unwrap();
    let shift_table = fs::read_to_string("./templates/shift_table.html").unwrap();
    info!("Sending removed shifts mail");
    let enkelvoud_meervoud = if removed_shifts.len() == 1 {
        "is"
    } else {
        "zijn"
    };
    let email_shift_s = if removed_shifts.len() == 1 { "" } else { "en" };
    let name = get_set_name(None);
    let mut shift_tables = String::new();
    for shift in &removed_shifts {
        let shift_table_clone = strfmt!(&shift_table,
            shift_number => shift.number.clone().strikethrough(),
            shift_date => shift.date.format(DATE_DESCRIPTION)?.to_string().strikethrough(),
            shift_start => shift.start.format(TIME_DESCRIPTION)?.to_string().strikethrough(),
            shift_end => shift.end.format(TIME_DESCRIPTION)?.to_string().strikethrough(),
            shift_duration_hour => shift.duration.whole_hours().to_string().strikethrough(),
            shift_duration_minute => (shift.duration.whole_minutes() % 60).to_string().strikethrough(),
            shift_link => create_shift_link(shift, false).unwrap_or_default(),
            bussie_login => if let Ok(url) = create_footer(true) {format!("/loginlink/{url}")} else {String::new()},
            shift_link_pdf => create_shift_link(shift, true).unwrap_or_default()
        )?;
        shift_tables.push_str(&shift_table_clone);
    }
    let removed_shift_html = strfmt!(&removed_shift_html,
        name => name.clone(),
        shift_changed_ammount => removed_shifts.len().to_string(),
        single_plural_en => email_shift_s,
        single_plural => enkelvoud_meervoud,
        shift_tables
    )?;
    let email_body_html = strfmt!(&base_html,
        content => removed_shift_html,
        banner_color => COLOR_BLUE,
        footer => create_footer(false).unwrap_or_default()
    )?;
    let email = Message::builder()
        .from(format!("{} <{}>", SENDER_NAME, &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to).parse()?)
        .subject(&format!(
            "{} dienst{} {} verwijderd",
            removed_shifts.len(),
            email_shift_s,
            enkelvoud_meervoud
        ))
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

/*
Composes and sends email of found errors, in plaintext
List of errors can be as long as possible, but for now is always 3
*/
pub fn send_errors(errors: &Vec<GenError>, name: &str) -> GenResult<()> {
    let env = EnvMailVariables::new();
    if !env.send_error_mail {
        info!("tried to send error mail, but is disabled");
        return Ok(());
    }
    warn!(
        "Er zijn fouten opgetreden, mailtje met fouten wordt gestuurd naar {}",
        &env.mail_error_to
    );
    let mailer = load_mailer(&env)?;
    let mut email_errors = "Er zijn fouten opgetreden tijdens het laden van shifts\n".to_string();
    for error in errors {
        email_errors.push_str(&format!("Error: \n{}\n\n", error.to_string()));
    }
    let email = Message::builder()
        .from(format!("Foutje Berichtmans <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_error_to).parse()?)
        .subject(&format!("Fout bij laden shifts van: {}", name))
        .header(ContentType::TEXT_PLAIN)
        .body(email_errors)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_gecko_error_mail<T: std::fmt::Debug>(error: WebDriverResult<T>) -> GenResult<()> {
    let env = EnvMailVariables::new();
    if !env.send_error_mail {
        info!("tried to send GECKO error mail, but is disabled");
        return Ok(());
    }
    let mailer = load_mailer(&env)?;
    let mut email_errors = "!!! KAN NIET VERBINDEN MET GECKO !!!\n".to_string();
    email_errors.push_str(&format!(
        "Error: \n{}\n\n",
        error
            .err()
            .unwrap_or(thirtyfour::error::WebDriverError::UnknownError(
                WebDriverErrorInfo::new("Unknown".to_owned())
            ))
            .to_string()
    ));
    let email = Message::builder()
        .from(format!("Foutje Berichtmans <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", "user", &env.mail_error_to).parse()?)
        .subject(&format!("KAN NIET VERBINDEN MET GECKO"))
        .header(ContentType::TEXT_PLAIN)
        .body(email_errors)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_welcome_mail(path: &PathBuf, force: bool) -> GenResult<()> {
    if path.exists() && !force {
        return Ok(());
    }

    let env = EnvMailVariables::new();

    if !env.send_welcome_mail && !force {
        info!("Wanted to send welcome mail. But it is disabled");
        return Ok(());
    }

    let mailer = load_mailer(&env)?;
    let (_user, properties) = get_data();

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let onboarding_html = fs::read_to_string("./templates/onboarding_base.html").unwrap();

    let name = get_set_name(None);

    let agenda_url = create_footer(true).unwrap_or(ERROR_VALUE.to_owned());
    let agenda_url_webcal = agenda_url.clone().replace("https", "webcal");
    // A lot of email clients don't want to open webcal links. So by pointing to a website which returns a 302 to a webcal link it tricks the email client
    let rewrite_url = &properties.webcal_domain;
    let webcal_rewrite_url = format!(
        "{rewrite_url}{}",
        if !rewrite_url.is_empty() {
            create_ical_filename()
        } else {
            agenda_url_webcal.clone()
        }
    );
    let kuma_url = &properties.kuma_properties.domain;
    let kuma_info = if !kuma_url.is_empty() {
        let extracted_kuma_mail = &properties
            .kuma_properties
            .kuma_email_properties
            .mail_from
            .split("<")
            .last()
            .unwrap_or_default()
            .replace(">", "");
        format!(
            "Als Webcom Ical een storing heeft ontvang je meestal een mail van <em>{}</em> (deze kan in je spam belanden!), op <a href=\"{kuma_url}\" style=\"color:#d97706;text-decoration:none;\">{kuma_url}</a> kan je de actuele status van Webcom Ical bekijken.",
            extracted_kuma_mail
        )
    } else {
        "".to_owned()
    };
    let donation_properties = properties.donation_text.clone();
    let donation_text = donation_properties.donate_text;
    let donation_service = donation_properties.donate_service_name;
    let donation_link = donation_properties.donate_link;
    let iban = donation_properties.iban;
    let iban_name = donation_properties.iban_name;
    let admin_email = env.mail_error_to;
    let onboarding_html = strfmt!(&onboarding_html,
        name => name.clone(),
        agenda_url,
        agenda_url_webcal,
        webcal_rewrite_url,
        kuma_info,
        donation_service,
        donation_text,
        donation_link,
        iban,
        iban_name,
        admin_email
    )?;
    let email_body_html = strfmt!(&base_html,
        content => onboarding_html,
        banner_color => COLOR_BLUE,
        footer => "".to_owned()
    )?;
    warn!("welkom mail sturen");
    let email = Message::builder()
        .from(format!("{} <{}>", SENDER_NAME, &env.mail_from).parse()?)
        .to(format!("{} <{}>", name, &env.mail_to).parse()?)
        .subject(format!("Welkom bij Webcom Ical {}!", &name))
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_failed_signin_mail(
    error: &IncorrectCredentialsCount,
    first_time: bool,
) -> GenResult<()> {
    let env = EnvMailVariables::new();
    if !env.send_failed_signin_mail {
        return Ok(());
    }

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let login_failure_html = fs::read_to_string("./templates/failed_signin.html").unwrap();
    let (_user, properties) = get_data();
    info!("Sending failed sign in mail");
    let mailer = load_mailer(&env)?;
    let still_not_working_modifier = if first_time { "" } else { "nog steeds " };
    let name = get_set_name(None);
    let verbose_error = match &error.error {
        Some(SignInFailure::IncorrectCredentials) => {
            "Incorrecte inloggegevens, heb je misschien je wachtwoord veranderd?"
        }
        Some(SignInFailure::TooManyTries) => "Te veel incorrecte inlogpogingen…",
        Some(SignInFailure::WebcomDown) => "Webcom heeft op dit moment een storing",
        Some(SignInFailure::Other(fault)) => fault,
        _ => "Een onbekende fout...",
    };
    let password_reset_link = &properties.password_reset_link;
    let password_change_text = if error
        .error
        .clone()
        .is_some_and(|error| error == SignInFailure::IncorrectCredentials)
    {
        format!("
<tr>
    <td>
        Als je je webcomm wachtwoord hebt veranderd. Vul je nieuwe wachtwoord in met behulp van de volgende link: <br>
        <a href=\"{password_reset_link}\" style=\"color:#003366; text-decoration:underline;\">{password_reset_link}</a>
    </td>
</tr>")
    } else {
        String::new()
    };

    let login_failure_html = strfmt!(&login_failure_html,
        still_not_working_modifier,
        name => get_set_name(None),
        additional_text => password_change_text,
        retry_counter => error.retry_count,
        signin_error => verbose_error.to_string(),
        admin_email => env.mail_error_to.clone(),
        name => name.clone()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => login_failure_html,
        banner_color => COLOR_RED,
        footer => create_footer(false).unwrap_or_default()
    )?;

    let email = Message::builder()
        .from(format!("WEBCOM ICAL <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to).parse()?)
        .subject("INLOGGEN WEBCOM NIET GELUKT!")
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_sign_in_succesful() -> GenResult<()> {
    let env = EnvMailVariables::new();

    if !env.send_error_mail {
        return Ok(());
    }

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let login_success_html = fs::read_to_string("./templates/signin_succesful.html").unwrap();
    let name = get_set_name(None);
    info!("Sending succesful sign in mail");

    let mailer = load_mailer(&env)?;
    let sign_in_email_html = strfmt!(&login_success_html,
        name => name.clone()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => sign_in_email_html,
        banner_color => COLOR_GREEN,
        footer => create_footer(false).unwrap_or_default()
    )?;

    let email = Message::builder()
        .from(format!("WEBCOM ICAL <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", name, &env.mail_to).parse()?)
        .subject("Webcom Ical kan weer inloggen!")
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_new_shift_mail() -> GenResult<()> {
        let shift = create_example_shift();
        let (env, mailer) = get_mailer()?;
        create_send_new_email(&mailer, vec![&shift, &shift], &env, false)
    }

    #[test]
    fn send_updated_shift_mail() -> GenResult<()> {
        let shift = create_example_shift();
        let (env, mailer) = get_mailer()?;
        create_send_new_email(&mailer, vec![&shift, &shift], &env, true)
    }

    #[test]
    fn send_deleted_shift_mail() -> GenResult<()> {
        let shift = create_example_shift();
        let (env, mailer) = get_mailer()?;
        send_removed_shifts_mail(&mailer, &env, vec![&shift, &shift])
    }

    #[test]
    fn send_welcome_mail_test() -> GenResult<()> {
        send_welcome_mail(&PathBuf::new(), true)
    }

    #[test]
    fn send_failed_signin_test() -> GenResult<()> {
        let credential_error = IncorrectCredentialsCount {
            retry_count: 30,
            error: Some(SignInFailure::IncorrectCredentials),
            previous_password_hash: None,
        };
        send_failed_signin_mail(&credential_error, false)
    }

    #[test]
    fn send_succesful_sign_in() -> GenResult<()> {
        send_sign_in_succesful()
    }

    fn create_example_shift() -> Shift {
        Shift::new("Dienst: V2309 •  • Geldig vanaf: 29.06.2025 •  • Tijd: 06:14 - 13:54 •  • Dienstduur: 07:40 Uren •  • Loonuren: 07:40 Uren •  • Dagsoort:  • Donderdag •  • Dienstsoort:  • Rijdienst •  • Startplaats:  • ehvgas, Einhoven garage streek •  • Omschrijving:  • V".to_owned(),Date::from_calendar_date(2025, time::Month::June, 29).unwrap()).unwrap()
    }

    fn get_mailer() -> GenResult<(EnvMailVariables, SmtpTransport)> {
        let env = EnvMailVariables::new();
        let mailer = load_mailer(&env)?;
        Ok((env, mailer))
    }
}
