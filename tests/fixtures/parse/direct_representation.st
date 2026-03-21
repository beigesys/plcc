PROGRAM DirectRep
VAR
    input_word AT %IW0 : WORD;
    output_byte AT %QB0 : BYTE;
    memory_bit AT %MX0.0 : BOOL;
    memory_dword AT %MD10 : DWORD;
END_VAR
    output_byte := 16#FF;
    memory_bit := TRUE;
END_PROGRAM
