# relay

Line-protocol TCP chat with rooms. `/nick NAME`, `/join ROOM`, anything
else broadcasts to your room; joins, renames, and departures are
announced. Per-client writer threads keep one stalled reader from
blocking a room. Tests drive two-and-three real socket clients through
nick changes, echo, and room isolation.

    relay serve 4000
    relay join host:4000
