use crate::{
    GenResult, Shift,
    email::{DATE_DESCRIPTION, TIME_DESCRIPTION},
    errors::ResultLog,
    get_instance,
    shift::ShiftState,
};
use thirtyfour::{WebDriver, WebElement, error::WebDriverResult, prelude::*};
use time::{Duration, Time};

/*
Main function for loading broken shifts
First visits the web page
Finds the op- and afstaptijden
Creates two new shifts and adds them to the list
Returns the new list
Does not return most errors as there are a few valid reason this function fails
*/
pub async fn load_broken_shift_information(
    driver: &WebDriver,
    all_shifts: &Vec<Shift>,
) -> GenResult<Vec<Shift>> {
    let mut shifts_clone = all_shifts.clone();
    for shift in shifts_clone.iter_mut() {
        if !shift.is_broken {
            continue;
        }
        // Try to load the broken shift information. If it fails, that is not important
        if shift.broken_period.is_none() {
            info!("Creating broken shift: {}", shift.number);
            load_single_broken_info(driver, shift).await?;
        } else if matches!(shift.state, ShiftState::Changed | ShiftState::New) {
            info!("Creating broken shift: {}", shift.number);
            load_single_broken_info(driver, shift).await?;
        } else {
            info!(
                "Shift {} is broken, but unchanged from last check",
                shift.number
            );
        }
    }
    info!("Done generating broken shifts");
    Ok(shifts_clone)
}

async fn load_single_broken_info(driver: &WebDriver, shift: &mut Shift) -> GenResult<()> {
    match get_broken_shift_time(driver, shift).await {
        Ok(_) => {
            info!("Added broken shift time to shift {}", shift.number);
        }
        Err(x) => {
            warn!(
                "An error occured creating a broken shift {}: {}",
                shift.number,
                x.to_string()
            );
        }
    };
    navigate_to_subdirectory(driver, "/WebComm/roster.aspx").await?; //Ga terug naar de rooster pagina, anders laden de gebroken shifts niet goed
    wait_for_response(driver, By::ClassName("calDay"), false).await?;
    Ok(())
}

/*
A small function to combine the three functions needed for creating a broken shift into one match statement
*/
async fn get_broken_shift_time(driver: &WebDriver, shift: &mut Shift) -> GenResult<()> {
    let broken_diensten = load_broken_dienst_page(driver, &shift).await?;
    let between_times = find_broken_start_stop_time(broken_diensten).await?;
    shift.broken_period = Some(between_times);
    Ok(())
}

/*
Looks for a time difference between one event in the shift info and the next
If something boes wrong, skip it.
*/
pub async fn find_broken_start_stop_time(
    shift_rows: Vec<WebElement>,
) -> GenResult<Vec<(Time, Time)>> {
    let mut broken_periods: Vec<(Time, Time)> = vec![];
    let mut previous_element_end_time = None;
    for activity in shift_rows {
        let activity_columns = activity.query(By::Tag("td")).all_from_selector().await?;
        let (activity_start_time, activity_end_time) = match async || -> GenResult<(Time, Time)> {
            Ok((
                Time::parse(&activity_columns[1].text().await?, TIME_DESCRIPTION)?,
                Time::parse(&activity_columns[3].text().await?, TIME_DESCRIPTION)?,
            ))
        }()
        .await
        .warn_owned("Getting broken shift element time")
        {
            Ok(times) => times,
            Err(_) => {
                // If anything goes wrong getting the time information of the element, just skip it.
                // By setting it to none it wont be used with the next element
                previous_element_end_time = None;
                continue;
            }
        };
        if let Some(previous_time) = previous_element_end_time {
            let time_difference = activity_start_time - previous_time;
            if time_difference > Duration::minutes(10) {
                broken_periods.push((previous_time, activity_start_time));
            }
        }
        previous_element_end_time = Some(activity_end_time);
    }
    debug!("Broken periods found: {broken_periods:?}");
    Ok(broken_periods)
}

/*
A function created to overcome a limitation of gnome calendar
https://gitlab.gnome.org/GNOME/gnome-calendar/-/issues/944
Not needed for most people
*/
pub fn split_night_shift(shifts: &Vec<Shift>) -> GenResult<Vec<Shift>> {
    let (user, _properties) = get_instance();
    let split = user.user_properties.split_night_shift;
    let mut temp_shift: Vec<Shift> = vec![];
    if !split {
        temp_shift = shifts.clone();
        return Ok(temp_shift);
    }
    for shift in shifts {
        if shift.end_date != shift.date {
            let split_shift = Shift::new_from_existing(
                (
                    Time::from_hms(0, 0, 0).unwrap(),
                    Time::from_hms(0, 0, 0).unwrap(),
                ),
                shift,
                true,
            );
            temp_shift.extend(split_shift);
        } else {
            temp_shift.push(shift.clone());
        }
    }
    Ok(temp_shift)
}

// Function to stop shifts at midnight. This is a request from Jerry
pub fn stop_shift_at_midnight(shifts: &Vec<Shift>) -> GenResult<Vec<Shift>> {
    let (user, _properties) = get_instance();
    let stop = user.user_properties.stop_midnight_shift;
    let mut temp_shifts: Vec<Shift> = vec![];
    if !stop {
        return Ok(shifts.clone());
    }
    for shift in shifts {
        let mut shift_clone = shift.clone();
        if shift.end_date != shift.date {
            shift_clone.original_end_time = Some(shift_clone.end);
            shift_clone.end = Time::from_hms(23, 59, 0).unwrap();
            shift_clone.end_date = shift_clone.date;
        }
        temp_shifts.push(shift_clone);
    }
    Ok(temp_shifts)
}

/*
Creates the URL needed for the broken shift
Waits untill the page is fully loaded and then gets and returns all rows of the shift sheet
*/
pub async fn load_broken_dienst_page(
    driver: &WebDriver,
    shift: &Shift,
) -> GenResult<Vec<WebElement>> {
    let date = shift.date;
    let formatted_date = date.format(DATE_DESCRIPTION)?;
    navigate_to_subdirectory(driver, &format!("/WebComm/shift.aspx?{}", formatted_date)).await?;
    //wait_until_loaded(&driver).await?;
    wait_for_response(driver, By::PartialLinkText("Werk en afwezigheden"), true).await?;
    let trip_body = driver.find(By::Tag("tbody")).await?;
    let trip_rows = trip_body.query(By::Tag("tr")).all_from_selector().await?;
    Ok(trip_rows)
}

/*
A function to navigate to a subdirectory of the current URL
Needed because if the while url is entered, the cookies will be lost and you will have to log in again
*/
pub async fn navigate_to_subdirectory(
    driver: &WebDriver,
    subdirectory: &str,
) -> WebDriverResult<()> {
    let script = format!("window.location.href = '{}';", subdirectory);
    driver.execute(&script, vec![]).await?;
    Ok(())
}

// This function clones a vec of shifts and splits broken shifts, if that value is set
pub fn split_broken_shifts(shifts: &Vec<Shift>) -> Vec<Shift> {
    let mut shifts_clone: Vec<Shift> = shifts.iter().cloned().collect();
    let mut shifts_to_append = vec![];
    let vec_len = shifts_clone.len() - 1;
    for shift in shifts.iter().rev().enumerate() {
        let position = vec_len - shift.0;
        if shift.1.broken_period.is_some() {
            if let Some(mut shifts_split) = shift.1.split_broken() {
                debug!(
                    "Broken shift {} has broken shift times of {:?}",
                    shift.1.number, shift.1.broken_period
                );
                shifts_clone.remove(position);
                shifts_to_append.append(&mut shifts_split);
            }
        }
    }
    shifts_clone.append(&mut shifts_to_append);
    shifts_clone
}

/*
A simple function to wait until a page is truly fully loaded
You need to provide a element on the page to wait for
If clickable is false it will only check if it is displayed, not clickable
*/
pub async fn wait_for_response(
    driver: &WebDriver,
    element: By,
    clickable: bool,
) -> WebDriverResult<()> {
    let query = driver.query(element.clone()).first().await?;
    match clickable {
        true => {
            query
                .wait_until()
                .wait(
                    std::time::Duration::from_secs(60),
                    std::time::Duration::from_secs(1),
                )
                .clickable()
                .await?
        }
        false => {
            query
                .wait_until()
                .wait(
                    std::time::Duration::from_secs(60),
                    std::time::Duration::from_secs(1),
                )
                .displayed()
                .await?
        }
    };
    Ok(())
}
