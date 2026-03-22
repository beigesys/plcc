VAR_GLOBAL
    g_counter : INT;
    g_flag : BOOL;
END_VAR

PROGRAM GlobalTest
VAR
    local_copy : INT;
END_VAR
    g_counter := g_counter + 1;
    local_copy := g_counter;
END_PROGRAM
