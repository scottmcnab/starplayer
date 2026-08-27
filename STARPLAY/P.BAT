tasm /ml /m2 star.asm
@rem tasm /ml /m2 cdromlib.asm
tasm /ml /m2 s3mlib.asm
wlink @pmodew.lnk system pmodew file star file cdromlib file s3mlib
pmwbind /R /O /Sdos4gw.exe star.exe
