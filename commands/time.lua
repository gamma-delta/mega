-- os.date turns a time into a string with a format
-- "It is Wednesday 27 May 2020, at 2 06 pm"
local formatted_time = os.date("It is %A %d %B %Y, at %I %M %p")
print(os.time())

Mega.speak(formatted_time)