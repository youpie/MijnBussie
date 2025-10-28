use crate::{
    GenResult,
    email::send_errors,
    errors::{FailureType, ResultLog},
    get_set_name,
    health::{ApplicationLogbook, send_heartbeat},
};
use dotenvy::var;
use thirtyfour::{DesiredCapabilities, WebDriver, error::WebDriverError};
use tracing::*;

pub async fn initiate_webdriver() -> GenResult<WebDriver> {
    let gecko_ip = var("SELENIUM_URL")?;
    let caps = DesiredCapabilities::firefox();
    let driver = WebDriver::new(format!("http://{}", gecko_ip), caps).await?;
    Ok(driver)
}

pub async fn get_driver(logbook: &mut ApplicationLogbook) -> GenResult<WebDriver> {
    match initiate_webdriver().await {
        Ok(driver) => Ok(driver),
        Err(error) => {
            error!("Kon driver niet opstarten: {:?}", &error);
            send_errors(&vec![error], &get_set_name(None)).info("Send errors");
            logbook
                .save(&FailureType::GeckoEngine)
                .warn("Saving Logbook");
            send_heartbeat(&FailureType::GeckoEngine)
                .await
                .warn("Sending heartbeat");
            return Err("driver fout".into());
        }
    }
}

pub async fn wait_until_loaded(driver: &WebDriver) -> GenResult<()> {
    let mut started_loading = false;
    let timeout_duration = std::time::Duration::from_secs(30);
    let _ = tokio::time::timeout(timeout_duration, async {
        loop {
            let ready_state = driver.execute("return document.readyState", vec![]).await?;
            let current_state = format!("{:?}", ready_state.json());
            if current_state == "String(\"complete\")" && started_loading {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                return Ok::<(), WebDriverError>(());
            }
            if current_state == "String(\"loading\")" {
                started_loading = true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

pub async fn wait_untill_redirect(driver: &WebDriver) -> GenResult<()> {
    let initial_url = driver.current_url().await?;
    let mut current_url = driver.current_url().await?;
    let timeout = std::time::Duration::from_secs(30); // Maximum wait time.

    tokio::time::timeout(timeout, async {
        loop {
            let new_url = driver.current_url().await.unwrap();
            if new_url != current_url {
                current_url = new_url;
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;

    if current_url == initial_url {
        warn!("Timeout waiting for redirect.");
        return Err(Box::new(WebDriverError::Timeout(
            "Redirect did not occur".into(),
        )));
    }

    debug!("Redirected to: {}", current_url);
    wait_until_loaded(driver).await?;
    Ok(())
}
