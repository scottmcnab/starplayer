tasm /ml /m2 cdplay.asm
tasm /ml /m2 cdromlib.asm
wlink @pmodew.lnk system pmodew file cdplay,cdromlib
pmwbind /R /O /Sdos4gw.exe cdplay.exe
