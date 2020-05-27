-- A mapping of number words to actual numbers
local number_words = {
    one = 1,
    two = 2,
    three = 3,
    four = 4,
    five = 5,
    six = 6,
    seven = 7,
    eight = 8,
    nine = 9,
    ten = 10,
    eleven = 11,
    twelve = 12,
    thirteen = 13,
    fourteen = 14,
    fifteen = 15,
    sixteen = 16,
    seventeen = 17,
    eighteen = 18,
    nineteen = 19,
    twenty = 20
}

-- By default roll 1d6
local count = 1
local size = 6

-- Check if I'm asking for more dice
if #Mega.raw_arguments >= 1 then
    for _, count_arg in pairs(Mega.raw_arguments[1]) do
        local maybe_count = number_words[count_arg]
        if maybe_count ~= nil then
            count = maybe_count
            break
        end
    end
end

-- Ignore the second argument (hopefully it's "dee")

-- Check for size of dice
if #Mega.raw_arguments >= 3 then
    for _, size_arg in pairs(Mega.raw_arguments[3]) do
        local maybe_size = number_words[size_arg]
        if maybe_size ~= nil then
            size = maybe_size
            break
        end
    end
end

Mega.speak(string.format("Rolling %d dee %d...", count, size))
local total = 0
for _ = 1,count do
    total = total + math.random(size)
end
-- wait a little bit so it has time to speak
local sec = os.clock() + 2.5
while os.clock() < sec do end -- a busy loop, yay

Mega.speak(string.format("Rolled %d", total))