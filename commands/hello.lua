if #Mega.raw_arguments == 0 then
    Mega.speak "Hello."
else
    local found_mega = false
    for _, v in pairs(Mega.raw_arguments[1]) do
        print("a raw argument: ", v)
        if v == "mega" then
            found_mega = true
            break
        end
    end
    if found_mega then
        Mega.speak "Hello, user."
    else
        Mega.speak "That's not my name!"
    end
end