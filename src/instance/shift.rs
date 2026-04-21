use std::{
    collections::HashMap, hash::{DefaultHasher, Hash, Hasher}, str::Split
};

use serde::{Deserialize, Serialize};
use serde_with::{DefaultOnError, serde_as};
use time::{Date, Duration, Time, macros::format_description};

use crate::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum ShiftState {
    New,
    Changed,
    Deleted,
    Unchanged,
    #[default]
    Unknown,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Shift {
    pub date: Date,
    pub start: Time,
    pub end_date: Date,
    pub end: Time,
    pub duration: Duration,
    pub number: String,
    pub kind: String,
    pub location: String,
    pub description: String,
    pub is_broken: bool,
    // If the shift is broken, between what times is the user free
    // If none, something went wrong
    #[serde(default)]
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub broken_period: Option<Vec<(Time, Time)>>,
    pub original_end_time: Option<Time>,
    pub magic_number: i64,
    // This field is not always needed. Especially when serializing.
    #[serde(skip_deserializing, default)]
    pub state: ShiftState,
}

impl Shift {
    /*
    Creates a new Shift struct from a simple string straight from webcom
    Also hashes the string to see if it has been updated
    Looks intimidating, bus is mostly boilerplate + a bit of logic for correctly parsing the duration
    */
    pub fn new(text: String, date: Date) -> Result<Self> {
        let text_clone = text.clone();
        let parts = text_clone.split("\u{a0}• \u{a0}• ");
        let mut location_modifier = 1;
        let parts_clean: Vec<String> = parts
            .map(|x| {
                let y = x.replace("\u{a0}• ", "");
                y
            })
            .collect();
        let mut parts_list: Vec<Split<'_, &str>> =
            parts_clean.iter().map(|x| x.split(": ")).collect();
        let number: String = parts_list[0].nth(1).result()?.to_string();
        let _date: String = parts_list[1].nth(1).result()?.to_string();
        let time: String = parts_list[2].nth(1).unwrap_or("").to_string();
        let shift_duration: String = parts_list[3].nth(1).unwrap_or("").to_string();
        let _working_hours: String = parts_list[4].nth(1).unwrap_or("").to_string();
        let _day_of_week: String = parts_list[5].nth(1).unwrap_or("").to_string();
        let kind: String = parts_list[6].nth(1).unwrap_or("").to_string();
        let mut location = "Onbekend".to_string();
        if parts_list[7].next().unwrap_or("") == "Startplaats" {
            location_modifier = 0;
            location = parts_list[7].next().unwrap_or("").to_string();
        }
        let description: String = parts_list[8 - location_modifier]
            .nth(1)
            .unwrap_or("")
            .to_string();
        let start_time_str = time.split_whitespace().nth(0).result()?;
        let end_time_str = time.split_whitespace().nth(2).result()?;
        let start = Shift::get_time(start_time_str)?;
        let end = Shift::get_time(end_time_str)?;
        let mut is_broken = false;
        let shift_type = number.chars().nth(0).result()?;
        let mut hasher = DefaultHasher::new();
        let hash_list = (date, &number, &start, &end, &shift_duration);
        hash_list.hash(&mut hasher);
        let magic_number = (hasher.finish() as i128 - i64::MAX as i128) as i64;
        if shift_type == 'g' || shift_type == 'G' {
            is_broken = true;
        }

        let duration_split = shift_duration
            .split_whitespace()
            .nth(0)
            .result()?
            .split(":");
        let duration_minutes =
            Duration::minutes(duration_split.clone().nth(1).result()?.parse::<i64>()?);
        let duration_hours =
            Duration::hours(duration_split.clone().nth(0).result()?.parse::<i64>()?);
        let duration = duration_hours + duration_minutes;
        let mut end_date = date;
        if end < start {
            end_date = date + Duration::days(1);
        }
        Ok(Self {
            date,
            number,
            start,
            end_date,
            end,
            duration,
            kind,
            location,
            description,
            is_broken,
            broken_period: None,
            original_end_time: None,
            magic_number,
            state: ShiftState::Unknown,
        })
    }

    // Create new shifts from one broken shift.
    // Assumes second shift cannot start after midnight
    // None means no broken times have been found for the shift
    pub fn split_broken(&self) -> Option<Vec<Self>> {
        if let Some(broken_periods) = self.broken_period.as_deref()
            && !broken_periods.is_empty()
        {
            let mut split_shifts = vec![];
            for period in broken_periods {
                let mut part_one = self.clone();
                part_one.end = period.0;
                let mut part_two = self.clone();
                part_two.start = period.1;
                split_shifts.push(part_one);
                split_shifts.push(part_two);
            }
            Some(split_shifts)
        } else {
            None
        }
    }

    // Create two new shifts from one broken shift.
    // Assumes second shift cannot start after midnight
    pub fn new_from_existing(
        new_between_times: (Time, Time),
        existing_shift: &Self,
        start_next_day: bool,
    ) -> Vec<Self> {
        let mut part_one = existing_shift.clone();
        part_one.end = new_between_times.0;
        part_one.end_date = match start_next_day {
            true => existing_shift.end_date,
            false => existing_shift.date,
        };
        let mut part_two = existing_shift.clone();
        part_two.start = new_between_times.1;
        part_two.date = match start_next_day {
            true => existing_shift.end_date,
            false => existing_shift.date,
        };
        let shifts: Vec<Self> = vec![part_one, part_two];
        shifts
    }

    // Creates and returns a Time::time from a given string of time eg: 12:34
    fn get_time(str_time: &str) -> Result<Time> {
        let mut time_split = str_time.split(":");
        let mut hour: u8 = time_split.clone().next().result()?.parse()?;
        let min: u8 = time_split.nth(1).result()?.parse()?;
        if hour >= 24 {
            hour = hour - 24;
        }
        Ok(Time::from_hms(hour, min, 0)?)
    }

    pub fn create_shift_link(&self, include_domain: bool) -> Result<String> {
        let (_user, properties) = get_data();
        let date_format = format_description!("[day]-[month]-[year]");
        let formatted_date = self.date.format(date_format)?;
        let domain = match include_domain {
            true => &properties.pdf_shift_domain,
            false => "",
        };
        if domain.is_empty() && include_domain == true {
            return Ok(format!(
                "https://dmz-wbc-web01.connexxion.nl/WebComm/shiprint.aspx?{}",
                &formatted_date
            ));
        }
        let shift_number_bare = match self.number.split("-").next() {
            Some(shift_number) => shift_number,
            None => return Err(anyhow!("Could not get shift number")),
        };
        Ok(format!(
            "{domain}{shift_number_bare}?date={}",
            &formatted_date
        ))
    }
}

/*
Will search for new shifts given previous shifts.
Will be ran twice, If provided new shifts, it will look for updated shifts instead
Will send an email is send_mail is true
It doesn't make a lot of sense that this function is in Email
*/
pub fn attach_shift_status(
    new_shifts: Vec<Shift>,
    previous_shifts: Vec<Shift>,
    replace_old: bool,
) -> Vec<Shift> {
    if previous_shifts.is_empty() {
        // if the previous were empty, just return the list of current shifts as all new
        error!("!!! PREVIOUS SHIFTS WAS EMPTY. SKIPPING !!!");
        return new_shifts
            .into_iter()
            .map(|mut shift| {
                shift.state = ShiftState::New;
                shift
            })
            .collect();
    }

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
            if !replace_old {
                previous_shift.state = ShiftState::Unchanged;
            } else {
                new_shift.state = ShiftState::Unchanged;
                *previous_shift = new_shift
            }
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
    previous_shifts_map.into_values().collect()
}
