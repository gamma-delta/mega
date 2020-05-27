--- Mega's Lua API reference.  
--- This file doesn't actually do anything; all the commands and fields are handled by Rust. 
--- But, your IDE may be able to use the doc comments here to help you out.
--- @module Mega

-- make the linter happy
Mega = {}

--- Mega's best guess at the arguments passed to this command.  
--- For example, if the command is `utilities/time.lua`, and the user said "utilities time five six seven", 
--- this field would probably be `{"five", "six", "seven"}`.  
--- Speech-to-text is hard, though, so this table likely doesn't contain exactly what the user said.
--- See `raw_arguments` if you need finer data.
Mega.arguments = {}


--- The raw output of the speech-to-text algorithm.  
--- This has up to 20 guesses of what Mega thought might have been said for each argument. 
--- For example, if the user said the arguments "one, two, three", this field might be  
--- ```lua
--- {
---     {"won", "wan", "one", "un", ... },
---     {"too", "do", "two", "too", "sue", ...},
---     {"three", "free", "tea", "tech", ...}
--- }
--- ```
--- Speech-to-text is hard, isn't it?
Mega.raw_arguments = {}

--- Speak a string.
--- @param message string What to say
function Mega.speak(message) end