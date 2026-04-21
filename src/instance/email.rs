use anyhow::Context;
use lettre::{
    Message, SmtpTransport, Transport, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use secrecy::ExposeSecret;
use std::fs;
use strfmt::strfmt;

use crate::APPLICATION_NAME;
use crate::database::secret::Secret;

use super::data::get_set_name;
use super::deletion::DeletedReason;
use super::*;

const ERROR_VALUE: &str = "HIER HOORT WAT ANDERS DAN DEZE TEKST TE STAAN, CONFIGURATIE INCORRECT";
const SENDER_NAME: &str = "Peter";

pub const COLOR_BASE: &str = "#5F5AD3";
pub const COLOR_RED: &str = "#a51d2d";
pub const COLOR_GREEN: &str = "#26a269";

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
    pub mail_to: Secret,
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
pub fn send_emails(shifts: &Vec<Shift>) -> Result<()> {
    let env = EnvMailVariables::new();
    let mailer = load_mailer(&env)?;
    send_mail_changed_shifts(&mailer, &env, shifts)?;
    Ok(())
}

// Creates SMTPtransport from username, password and server found in env
fn load_mailer(env: &EnvMailVariables) -> GenResult<SmtpTransport> {
    let creds = Credentials::new(env.smtp_username.clone(), env.smtp_password.clone());
    let mailer = SmtpTransport::relay(&env.smtp_server)?
        .credentials(creds)
        .build();
    Ok(mailer)
}

fn send_mail_changed_shifts(
    mailer: &SmtpTransport,
    env: &EnvMailVariables,
    shifts: &Vec<Shift>,
) -> Result<()> {
    let current_date = time::OffsetDateTime::now_local()?.date();
    let mut new_shifts: Vec<&Shift> = shifts
        .iter()
        .filter(|item| item.state == ShiftState::New)
        .collect();
    let mut updated_shifts: Vec<&Shift> = shifts
        .iter()
        .filter(|item| item.state == ShiftState::Changed)
        .collect();
    let mut removed_shifts: Vec<&Shift> = shifts
        .iter()
        .filter(|item| item.state == ShiftState::Deleted)
        .collect();

    new_shifts.retain(|shift| shift.date >= current_date);
    if !new_shifts.is_empty() && env.send_email_new_shift {
        info!("Found {} new shifts, sending email", new_shifts.len());
        create_send_new_email(mailer, new_shifts, env, false)
            .context("Sending new shifts email")?;
    }
    updated_shifts.retain(|shift| shift.date >= current_date);
    if !updated_shifts.is_empty() && env.send_mail_updated_shift {
        info!(
            "Found {} updated shifts, sending email",
            updated_shifts.len()
        );
        create_send_new_email(mailer, updated_shifts, env, true)
            .context("Sending updated shifts email")?;
    }
    removed_shifts.retain(|shift| shift.date >= current_date);
    if !removed_shifts.is_empty() && env.send_removed_shift {
        info!("Removing {} shifts", removed_shifts.len());
        send_removed_shifts_mail(mailer, env, removed_shifts)
            .context("Sending removed shifts email")?;
    }
    Ok(())
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
            shift_link => shift.create_shift_link(false).unwrap_or_default(),
            bussie_login => if let Ok(url) = ical::create_calendar_link() {format!("/loginlink/{url}")} else {String::new()},
            shift_link_pdf => shift.create_shift_link(true).unwrap_or_default()
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
        banner_color => COLOR_BASE,
        footer => create_footer().unwrap_or(ERROR_VALUE.to_owned())
    )?;

    let email = Message::builder()
        .from(format!("Peter <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
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

fn create_footer() -> GenResult<String> {
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
    let admin_email = &properties.support_mail;
    Ok(    strfmt!(footer_text,
            footer_text => "Je agenda link:",
            footer_url => ical::create_calendar_link()?.to_string(),
            admin_email_comment => format!("Vragen of opmerkingen? Neem contact op met {admin_email}"))
        .unwrap_or_default())
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
            shift_link => shift.create_shift_link(false).unwrap_or_default(),
            bussie_login => if let Ok(url) = ical::create_calendar_link() {format!("/loginlink/{url}")} else {String::new()},
            shift_link_pdf => shift.create_shift_link(true).unwrap_or_default()
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
        banner_color => COLOR_BASE,
        footer => create_footer().unwrap_or_default()
    )?;
    let email = Message::builder()
        .from(format!("{} <{}>", SENDER_NAME, &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
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

pub fn send_welcome_mail(force: bool) -> GenResult<()> {
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

    let agenda_url = ical::create_calendar_link()?.to_string();
    let agenda_url_webcal = agenda_url.clone().replace("https", "webcal");
    // A lot of email clients don't want to open webcal links. So by pointing to a website which returns a 302 to a webcal link it tricks the email client
    let rewrite_url = &properties.webcal_domain;
    let webcal_rewrite_url = format!(
        "{rewrite_url}{}",
        if !rewrite_url.is_empty() {
            ical::create_ical_filename()
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
            "Als {APPLICATION_NAME} een storing heeft ontvang je meestal een mail van <em>{}</em> (deze kan in je spam belanden!), op <a href=\"{kuma_url}\" style=\"color:#d97706;text-decoration:none;\">{kuma_url}</a> kan je de actuele status van {APPLICATION_NAME} bekijken.",
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
        banner_color => COLOR_BASE,
        footer => "".to_owned()
    )?;
    warn!("welkom mail sturen");
    let email = Message::builder()
        .from(format!("{} <{}>", SENDER_NAME, &env.mail_from).parse()?)
        .to(format!("{} <{}>", name, &env.mail_to.0.expose_secret()).parse()?)
        .subject(format!("Welkom bij {APPLICATION_NAME} {}!", &name))
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_deletion_warning_mail() -> GenResult<()> {
    let env = EnvMailVariables::new();

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let warning_html = fs::read_to_string("./templates/potential_account_deletion.html").unwrap();
    let (_user, properties) = get_data();
    let mailer = load_mailer(&env)?;
    let name = get_set_name(None);
    let password_reset_link = &properties.password_reset_link;
    let calendar_id = ical::create_ical_filename();
    let password_change_text = create_new_password_form_html(password_reset_link, &calendar_id);

    let login_failure_html = strfmt!(&warning_html,
        name => get_set_name(None),
        additional_text => password_change_text,
        admin_email => env.mail_error_to.clone()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => login_failure_html,
        banner_color => COLOR_BASE,
        footer => String::new()
    )?;

    let email = Message::builder()
        .from(format!("{APPLICATION_NAME} <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
        .subject("Je Mijn Bussie account wordt over 7 dagen verwijderd")
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_account_deleted_mail(reason: DeletedReason) -> GenResult<()> {
    let env = EnvMailVariables::new();

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let deletion_html = fs::read_to_string("./templates/inform_account_deletion.html").unwrap();
    let (_user, properties) = get_data();
    let mailer = load_mailer(&env)?;
    let name = get_set_name(None);

    let login_failure_html = strfmt!(&deletion_html,
        name => get_set_name(None),
        deletion_reason => reason.to_str().to_owned(),
        visibility => match reason {
            DeletedReason::NewDead => "hidden",
            _ => "unset"
        }.to_owned(),
        sign_up_link => properties.sign_up_url.clone(),
        admin_email => env.mail_error_to.clone()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => login_failure_html,
        banner_color => COLOR_BASE,
        footer => String::new()
    )?;

    let email = Message::builder()
        .from(format!("{APPLICATION_NAME} <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
        .subject("Je Mijn Bussie is verwijderd")
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

pub fn send_incorrect_new_password_mail() -> GenResult<()> {
    let env = EnvMailVariables::new();
    if !env.send_failed_signin_mail {
        return Ok(());
    }

    let base_html = fs::read_to_string("./templates/email_base.html").unwrap();
    let new_password_fail_html =
        fs::read_to_string("./templates/new_password_failed.html").unwrap();
    let (_user, properties) = get_data();
    let mailer = load_mailer(&env)?;
    let name = get_set_name(None);
    let password_reset_link = &properties.password_reset_link;
    let calendar_id = ical::create_ical_filename();
    let password_change_text = create_new_password_form_html(password_reset_link, &calendar_id);

    let login_failure_html = strfmt!(&new_password_fail_html,
        name => get_set_name(None),
        additional_text => password_change_text,
        admin_email => env.mail_error_to.clone()
    )?;
    let email_body_html = strfmt!(&base_html,
        content => login_failure_html,
        banner_color => COLOR_RED,
        footer => create_footer().unwrap_or_default()
    )?;

    let email = Message::builder()
        .from(format!("{APPLICATION_NAME} <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
        .subject("Opgegeven Webcomm wachtwoord incorrect")
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
    let verbose_error = SignInFailure::to_string(error.error.as_ref());
    let password_reset_link = &properties.password_reset_link;
    let calendar_id = ical::create_ical_filename();
    let password_change_text = if error
        .error
        .clone()
        .is_some_and(|error| error == SignInFailure::IncorrectCredentials)
    {
        create_new_password_form_html(password_reset_link, &calendar_id)
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
        footer => create_footer().unwrap_or_default()
    )?;

    let email = Message::builder()
        .from(format!("{APPLICATION_NAME} <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", &name, &env.mail_to.0.expose_secret()).parse()?)
        .subject("INLOGGEN WEBCOM NIET GELUKT!")
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

fn create_new_password_form_html(password_reset_link: &str, calendar_id: &str) -> String {
    format!("
<tr>
    <td>
        Als je je webcomm wachtwoord hebt veranderd. Vul je nieuwe wachtwoord in met behulp van de volgende link: <br>
        <a href=\"{password_reset_link}?calendarId={calendar_id}\" style=\"color:#003366; text-decoration:underline;\">{password_reset_link}?calendarId={calendar_id}</a>
    </td>
</tr>")
}

pub fn send_sign_in_succesful() -> GenResult<()> {
    let env = EnvMailVariables::new();

    if !env.send_failed_signin_mail {
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
        footer => create_footer().unwrap_or_default()
    )?;

    let email = Message::builder()
        .from(format!("{APPLICATION_NAME} <{}>", &env.mail_from).parse()?)
        .to(format!("{} <{}>", name, &env.mail_to.0.expose_secret()).parse()?)
        .subject(format!("{APPLICATION_NAME} kan weer inloggen!"))
        .header(ContentType::TEXT_HTML)
        .body(email_body_html)?;
    mailer.send(&email)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use time::Date;

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
        send_welcome_mail(true)
    }

    #[test]
    fn send_new_password_incorrect_mail() -> GenResult<()> {
        send_incorrect_new_password_mail()
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
