use kuma_client::monitor::{MonitorGroup, MonitorType};
use kuma_client::{Client, monitor, notification};
use std::collections::HashMap;
use std::fs::read_to_string;
use std::str::FromStr;
use strfmt::strfmt;
use url::Url;

use crate::errors::OptionResult;
use crate::variables::{GeneralProperties, UserData, UserInstanceData};
use crate::watchdog::InstanceMap;
use crate::{GenResult, email};

const COLOR_RED: &str = "#a51d2d";
const COLOR_GREEN: &str = "#26a269";

const GROUP_NAME: &str = "MijnBussie";

pub async fn manage_users(instances_to_create: &Vec<String>,instances_to_remove: &Vec<String>,
    active_instances: &InstanceMap) -> GenResult<()> {
    for instance_name in instances_to_create {
        if let Some(instance) = active_instances.get(instance_name) {
            let (user, properties) = instance.user_instance_data.set_data_local().await;
            let kuma_properties = properties.kuma_properties;
            let client = connect_to_kuma(&Url::from_str(&kuma_properties.domain)?, &kuma_properties.username, &kuma_properties.password).await?;
        }
    }
    Ok(())
}

async fn connect_to_kuma(url: &Url, username: &str, password: &str) -> GenResult<Client> {
    Ok(Client::connect(kuma_client::Config {
        url: url.to_owned(),
        username: Some(username.to_owned()),
        password: Some(password.to_owned()),
        ..Default::default()
    })
    .await?)
}

async fn create_monitor(
    user: &UserData,
    properties: &GeneralProperties,
    kuma_client: &Client,
    user_name: &str,
    notification_id: i32,
) -> GenResult<i32> {

    let heartbeat_interval: i32 = (user.user_properties.execution_interval_minutes * 60)
        + properties.expected_execution_time_seconds;
    let heartbeat_retry: i32 = properties.kuma_properties.hearbeat_retry;
    let group_id: i32 = get_monitor_type_id(kuma_client, GROUP_NAME, MonitorType::Group)
        .await?
        .unwrap_or_default();
    let monitor = monitor::MonitorPush {
        name: Some(user_name.to_string()),
        interval: Some(heartbeat_interval),
        max_retries: Some(heartbeat_retry),
        retry_interval: Some(heartbeat_interval),
        push_token: Some(user_name.to_string()),
        notification_id_list: Some(HashMap::from([(notification_id.to_string(), true)])),
        parent: Some(group_id),
        ..Default::default()
    };
    let monitor_response = kuma_client.add_monitor(monitor).await?;
    let monitor_id = monitor_response.common().id().result()?;
    info!("Monitor has been created, id: {monitor_id}");
    Ok(monitor_id)
}

// Create a new notification if it does not already exist. The second value tells that a new notification has been created
async fn create_notification(
    user: &UserData,
    properties: &GeneralProperties,
    kuma_client: &Client,
    user_name: &str,
    kuma_url: &Url,
) -> GenResult<(i32, bool)> {
    let base_html =
        read_to_string("./templates/email_base.html").expect("Can't get email base template");
    let offline_html =
        read_to_string("./templates/kuma_offline.html").expect("Can't get kuma offline template");
    let online_html =
        read_to_string("./templates/kuma_online.html").expect("Can't get kuma online template");

    let body_online = strfmt!(&base_html,
        content => strfmt!(&online_html,
            name => set_get_name(None),
            kuma_url => kuma_url.to_string()
        )?,
        banner_color => COLOR_GREEN,
        footer => ""
    )?;
    let body_offline = strfmt!(&base_html,
        content => strfmt!(&offline_html,
            name => set_get_name(None),
            kuma_url => kuma_url.to_string(),
            msg => "{{msg}}"
        )?,
        banner_color => COLOR_RED,
        footer => ""
    )?;
    let body = format!(
        "{{% if status contains \"Up\" %}}
{body_online}
{{% else %}}
{body_offline}
{{% endif %}}"
    );
    debug!("Searching if notification already exists");
    let current_notifications = kuma_client.get_notifications().await?;
    for notification in current_notifications {
        if let Some(name) = notification.name
            && name == format!("{}_mail", user_name)
        {
            debug!(
                "Notification for user {user_name} already exists, ID: {:?}. Not creating new one",
                notification.id
            );
            return Ok((notification.id.result_reason("Getting notification ID")?, false));
        }
    }
    info!("Notification for user {user_name} does NOT yet exist, creating one");

    let email_env = email::EnvMailVariables::new_kuma();
    let port = properties.kuma_properties.mail_port;
    let secure = properties.kuma_properties.use_ssl;
    let config = serde_json::json!({
        "smtpHost": email_env.smtp_server,
        "smtpPort": port,
        "smtpUsername": email_env.smtp_username,
        "smtpPassword": email_env.smtp_password,
        "smtpTo": email_env.mail_to,
        "smtpFrom": email_env.mail_from,
        "customBody": body,
        "customSubject": "{% if status contains \"Up\" %}
MijnBussie storing verholpen!
{% else %}
MijnBussie heeft een storing
{% endif %}",
        "type": "smtp",
        "smtpSecure": secure,
        "htmlBody": true

    });
    let notification = notification::Notification {
        name: Some(format!("{}_mail", user_name.to_string())),
        config: Some(config),
        ..Default::default()
    };

    let notification_response = kuma_client.add_notification(notification.clone()).await?;
    let id = notification_response.id.result_reason("Getting new notification ID")?;
    info!("Created notification with ID {id}");
    Ok((id, true))
}

async fn get_monitor_type_id(
    kuma_client: &Client,
    group_name: &str,
) -> GenResult<Option<i32>> {
    let current_monitors = kuma_client.get_monitors().await?;
    // Check if a group with the same name of "group_name" exists
    for (_id, monitor) in current_monitors.into_iter() {
        if monitor.monitor_type() == MonitorType::Group {
            if monitor.common().name() == &Some(group_name.to_string()) {
                debug!(
                    "Existing monitor group has been found, ID: {:?}",
                    monitor.common().id()
                );
                return Ok(Some(monitor.common().id().result_reason("Getting monitor ID")?));
            }
        }
    }
    info!("Monitor group has not been found");
    // otherwise create a new one
    let new_monitor = kuma_client
        .add_monitor(MonitorGroup {
            name: Some(group_name.to_string()),
            ..Default::default()
        })
        .await?;
    let id = new_monitor.common().id().result_reason("Getting new monitor ID")?;
    info!(", created new one with id {id}");
    Ok(Some(id))
}
