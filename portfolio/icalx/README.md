# icalx

iCalendar toolkit: parse .ics (line unfolding, TEXT escaping, DATE and
DATE-TIME), expand recurrences, write spec-folded output. The RRULE subset
is documented, not pretended: FREQ daily/weekly/monthly/yearly, INTERVAL,
COUNT (counted from DTSTART), UNTIL, BYDAY for weekly, BYMONTHDAY for
monthly. Positional BYDAY (e.g. 2MO), BYSETPOS, and timezones beyond
UTC/floating are refused loudly. Floating times are treated as UTC —
stated. No chrono: civil math is Hinnant's algorithm, leap-year tested.

    icalx calendar.ics --from 20260101 --to 20261231
