# mailsift

mbox archive tool: `stats` (count, top senders), `search --from --subject
--since`, `export N` to .eml. Handles the parts old archives actually
contain: folded headers, `>From ` body quoting, RFC 2822 dates with zone
offsets (no chrono), and MIME encoded-word subjects (=?UTF-8?B?/Q?).
Charsets other than UTF-8 pass through raw rather than pretending.

    mailsift archive.mbox stats
    mailsift archive.mbox search --from alice --subject invoice
