local yes = {
    "I think so",
    "Calculations seem to suggest it",
    "Evaluates to true",
    "98.4% chance of success",
    "Positive"
}
local no = {
    "I think not",
    "Predictions suggest no",
    "Evaluating... false",
    "Incorrect",
    "Negative",
}

local success = math.random(0, 1) == 1
local responses = success and yes or no
Mega.speak(responses[math.random(#responses)])