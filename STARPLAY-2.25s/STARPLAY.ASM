; STARPLAY.ASM
; Main starplayer user interface code
; Copyright (c) Scott McNab (jedi/oxygen), 1994-1996
; jedi@tartarus.uwa.edu.au

		.386p
		jumps
		locals

;----------------------------------------------------------------------------
; Place all 16-bit code here

code16          segment para public use16
		assume cs:code16, ds:code16

JumpCode        proc far               ;Call rm int9
		pushf
		call dword ptr cs:[_OldRealMode]
		ret
JumpCode        endp

code16          ends

;----------------------------------------------------------------------------
code32          segment para public use32
		assume cs:code32, ds:code32

		include pmode.inc
		include file.inc
		include s3mlib.inc
		include cdromlib.inc

		public  _main

;----------------------------------------------------------------------------
; DATA
;----------------------------------------------------------------------------

CDSTEPTIME      equ     10      ;time to fast forward / rewind cd

Filename        db      80 DUP(?)
ActualFilename  db      13 DUP(?)
SeekFilename    db      80 DUP(?)
FILENAMESIZE    equ $-Filename

ParaBlock       dw      ?       ;Segment of environment
		dd      ?       ;ptr to command line
		dd      ?       ;ptr to FCB1
		dd      ?       ;ptr to FCB2

ParamString     db      ?,?

Temp_Palette    dd      ?

_Tmp_Min        db      ?
_Tmp_Sec        db      ?
_Tmp_Frame      db      ?

ActiveFlag      db      ?       ;Set this when scrollock pressed
PopUpFlag       db      ?       ;Set this when in popup menu

KeyboardLockFlag db     ?       ;disable keyboard flag

CDROM_Mode      db      ?       ;Set this when in CD audio mode
CDROM_Flag      db      ?       ;Set this if CDROM is installed
CDROM_Playing   db      ?       ;set if cdrom is playing
CDROM_Paused    db      ?       ;set if cdrom is paused
CDROM_Done      db      ?       ;set if cdrom is done
CDROM_StatusFlag dd     ?       ;flag for cdrom abilities

CDVolume        CD_Volumeinfo   ?

CDWasOpenFlag   db      ?       ;set this if cd tray is opened

CDPlayStatus    CD_Playinfo     ?

Menu_TopSlot    db      1       ;menu variables
Menu_Highlight  db      1
Menu_Size       db      ?

ListFileBuffer  dd      ?       ;pointer to listfile buffers
ListFileFree    dd      ?       ;bytes of listfilebuffer used
LISTSIZEMAX     EQU     8192    ;allow max 8k ext ram for listfiles

ListFlag        db      ?

ScanCode        db      ?       ;scanned key

Last_Row        db      25      ;last row of screen (50)

Comspec         db      'COMSPEC='
ComspecLen      equ     $-Comspec

ExitString      db      'type exit to return to starplayer.$'

_RealModeCode   db      21 dup(?)       ;Real mode irq handler
Old_Key_Handler dd      ?               ;Old keyboard handler
_OldRealMode    dd      ?               ;Old real mode irq handler

PlayerVolume    db      64              ;Global module volume

ProtectFlag     db      ?               ;Flag to prevent irq from interrupting
					;  foreground music code
Amplification   db      0               ;Variable to set forced amplification
ForceCard       db      DEVICE_AUTO     ;Force card type (default auto)

Default_MixRate dw      44100           ;Default mixing rate
Default_BufLen  dw      1024            ;Default buffer length

asciitbl        db      10 DUP(?)
Hex_values      db      '0123456789ABCDEF'

Scr_Xpos        db      0
Scr_Ypos        db      49

stflag          db      ?               ;load and play flag
foundcount      db      ?

DTASeg          dw      ?               ;segment and offset for DTA address
DTAOff          dw      ?

ExtraOffset     dd      ?               ;value to skip rows for scroll routine
ChangedFlag     dd      ?               ;flag for song state (byte diff)
ScreenBuffer    dd      ?               ;pointer to temporary screen buffer
ScreenBuffer2   dd      ?               ;pointer to menu screen buffer
ScreenBuffer3   dd      ?               ;pointer to file screen buffer
ScreenBuffer4   dd      ?               ;pointer to another menu buffer
ExtraData       dd      ?               ;pointer to temp himem buffer for menus
EXTRADATASIZE   equ     65+3            ;size of extra menu data

Init_Msg0       db      '-+ starplayer version 2.25s - source code release +-',0,'$'
Init_Msg1       db      'device: ',0
Init_Msg2       db      'base address: ',0
Init_Msg3       db      ' irq number: ',0
Init_Msg4       db      ' dma channel: ',0
Init_Msg6       db      ' mixing rate: ',0
Init_Msg7       db      ' dsp: ',0

Init_Msg8       db      'cdrom drive detected.',0

Letter_V        db      'v',0
Letter_Dot      db      '.',0

Info_Msg17      db      'Loading file: ',0

Help_Msg1       db      'starplayer quick help:',0
Help_Msg2       db      '  command   - starplay [<filename>]',0
		db      '  hot keys  Ú (with scroll lock on)',0
		db      '            ³ f1-f8      play multiple songs continuous',0
		db      '            ³ s-f1-f8    play one song looped',0
		db      '            ³ f9         terminate song',0
		db      '            ³ f10        popup song/file selection menu',0
		db      '            ³ f11,f12    inc, dec pattern',0
		db      '            ³ a-f11,f12  inc, dec volume',0
		db      '            À c-f11,f12  inc, dec current song',0
		db      255

CmdLne_Help1    db      13,10,'usage:   starplay <-options> <filespec>',13,10,10
		db      'where options are:',13,10
		db      '         -d        disables DMA sample loading (gus only)',13,10
		db      '         -m######  sets mixing rate in hz, default is 44100 (sb only)',13,10
		db      '         -b######  sets buffer size in bytes, default is 1024 (sb only)',13,10
		db      '         -a###     forces a fixed amplification, default is 48 (sb only)',13,10
		db      '         -c#       select a soundcard, default is autodetect (GUS=1, SB=2)',13,10
		db      '         -z##      select screen size, 25 or 50 rows',13,10,10
		db      '<filespec> can be any file, or a wildcard for multiple files',13,10,'$'

ErrorMsg        db      'ERROR: $'
ErrorMsg0       db      'Not enough extended ram.$'
ErrorMsg1       db      'Could not initialise sound device.',13,10
		db      'Ensure the ULTRASND= or BLASTER= environment variables',10,13
		db      'are set correctly or try resetting your sound card (ie. ULTRINIT)$'

ErrorTable      dd      offset ErrorMsg0
		dd      offset ErrorMsg1

Pan_Table       db      'LFT'           ;table for pan text
		db      ' 1 '
		db      ' 2 '
		db      ' 3 '
		db      ' 4 '
		db      ' 5 '
		db      ' 6 '
		db      ' 7 '
		db      ' 8 '
		db      ' 9 '
		db      ' A '
		db      ' B '
		db      ' C '
		db      ' D '
		db      ' E '
		db      'RGT'

NoteText        db      'C-','C#','D-','D#','E-','F-','F#','G-','G#'
		db      'A-','A#','B-'

VU_Colour_Table db      72h,72h,72h,72h,72h,72h,72h,72h
		db      7eh,7eh,7eh,7eh,7eh,7eh,7ch,7ch

FX_Info_Table   dd      0
		dd      offset FX_A
		dd      offset FX_B
		dd      offset FX_C
		dd      offset FX_D
		dd      offset FX_E
		dd      offset FX_F
		dd      offset FX_G
		dd      offset FX_H
		dd      offset FX_I
		dd      offset FX_J
		dd      offset FX_K
		dd      offset FX_L
		dd      0,0
		dd      offset FX_O
		dd      0
		dd      offset FX_Q
		dd      offset FX_R
		dd      0
		dd      offset FX_T
		dd      offset FX_U
		dd      offset FX_V
		dd      0
		dd      offset FX_X
		dd      0,0

FX_A            db      'change speed',0        ;a
FX_B            db      'jump to order',0       ;b
FX_C            db      'break pattern',0       ;c
FX_D            db      'volume slide',0        ;d
FX_E            db      'slide down',0          ;e
FX_F            db      'slide up',0            ;f
FX_G            db      'portamento',0          ;g
FX_H            db      'vibrato',0             ;h
FX_I            db      'tremor',0              ;i
FX_J            db      'arpeggio',0            ;j
FX_K            db      'vibrato & vol. slide',0;k
FX_L            db      'porta & vol. slide',0  ;l
FX_O            db      'sample offset',0       ;o
FX_Q            db      'note retrigger',0      ;q
FX_R            db      'tremolo',0             ;r
FX_T            db      'change tempo',0        ;t
FX_U            db      'fine vibrato',0        ;u
FX_V            db      'global volume',0       ;v
FX_X            db      'fine channel pan',0    ;x

SFX_Info_Table  dd      0
		dd      offset SFX_1
		dd      offset SFX_2
		dd      offset SFX_3
		dd      offset SFX_4
		dd      0
		dd      0
		dd      0
		dd      offset SFX_8
		dd      0
		dd      0
		dd      offset SFX_B
		dd      offset SFX_C
		dd      offset SFX_D
		dd      offset SFX_E
		dd      0

SFX_1           db      'glissando control',0   ;S1x
SFX_2           db      'set finetune',0        ;S2x
SFX_3           db      'set vibrato waveform',0;S3x
SFX_4           db      'set tremolo waveform',0;S4x
SFX_8           db      'channel pan',0         ;S8x
SFX_B           db      'pattern loop',0        ;SBx
SFX_C           db      'note cut',0            ;SCx
SFX_D           db      'note delay',0          ;SDx
SFX_E           db      'pattern delay',0       ;SEx

TypeTable       db      'S3M',0
		db      'MOD',0
		db      'MTM',0

Error_Msg1E     db      ?
Error_Msg3      db      'Error loading module.',0

MAXSLOTS        equ     64
Song_Table      dd      MAXSLOTS dup(?)         ;M_Struc_Size

CurrentSlot     db      ?       ;Current Load slot
CurrentSong     db      ?       ;Current song being played
LoopFlag        db      ?       ;Flag for module looping (0=enable)

DeviceTable     dd      offset _device_0
		dd      offset _device_1
		dd      offset _device_2

_device_0       db      'None',0
_device_1       db      'Gravis Ultrasound',0
_device_2       db      'Sound Blaster',0

;Filemenu stuff
PreDirectory    db      0,':\'
Directory       db      65 dup(?)
SeekPath        db      '*.*',0
FileFound       db      13 dup(?)
LastDrive       db      ?       ;last drive number

FMenu_TopSlot   db      1       ;file menu variables
FMenu_Highlight db      1
FUpdateFlag     db      1

FileList        dd      ?       ;pointer to file-list in mem
LastFilled      db      ?       ;last position filled in list
MAXLISTSIZE     equ     255     ;maximum list size

FileData        struc
_FileType       db      ?       ;1=s3m,2=mod,3=mtm,16=dir,32=drv,128=marked
_FileName       db      13 dup(?) ;dos filename
_FileTitle      db      28 dup(?) ;module title
_FileSize       dd      ?       ;size of file
FileDataSize    equ     $-_FileType     ;size of file list slot struc
FileData        ends

TxtCD_1         db      'open',0
TxtCD_2         db      'play',0

;Bitmaps
EndAnsi         label byte
	db      1,16,'Û²±°',25,7,15,'²',23,'°',7,16,'ÛÛÛ',25,6,15,26,8
	db      'Û',26,4,'ß Û ',1,26,6,'ß ',15,26,4,'Û  ',1,'ß',25,2,26
	db      5,'ß ',26,9,'ß±²Û',24,'Û±°',25,5,15,'²',23,'±°±',16,' '
	db      ' ',7,'Û²±° ',15,'°±²ÛÛ',23,'²²',16,'Û',25,8,'ÛÛ ÛÛ',25
	db      4,'ÛÛÛ',25,2,'ÛÛÛ',25,20,1,'°±Û',24,'Û°',25,4,15,'Û',23
	db      '²²±±',16,25,12,'Û',23,'±±',16,'Û',25,5,23,'±±²',16,25
	db      2,'ÛÛ',25,2,'ÛÛÛ',25,2,'ÛÛÛ',25,22,1,'°Û',24,'Û',25,3
	db      15,'²',26,10,'Û',25,6,23,'°°±±°',16,'°',25,3,23,'°±',7
	db      16,'Û',25,3,15,'ÛÛ²Û  ²',26,3,'Û',23,'²',16,25,6,11,'P'
	db      ' L A Y E R',25,7,1,'Û',24,'Û',25,13,15,26,3,'Û',25,4
	db      7,'Û',15,23,'°°±±',16,25,2,23,'°',7,16,'ÛÛÛßßß',25,2,15
	db      '²ÛÛÛ ²²ÛÛ Û',23,'²²',16,25,23,1,'Û',24,'Û  ',7,'°°±'
	db      '±²²ÛÛ',15,23,'°°±±²',16,26,4,'Û  ',7,'ÛÛ²²',15,23,'°'
	db      16,'±  ',7,'±²ÛÛ',25,5,'°',15,'±²Û',25,2,'²²ÛÛ  ',23,'±'
	db      '±',7,16,'Û',25,21,1,'Û',24,'Û ',7,'°±²',26,3,'Û',15,23
	db      '°°',26,3,'±²²',16,'ÛÛ',25,2,7,'ÛÛ²²±±',15,'°°  ',7,'±'
	db      '²ÛÛ',25,3,'°±²',15,'Û',25,3,7,'±±',15,'²²Û  ',23,'±'
	db      '°',7,16,'Û',8,23,'°',7,16,'Û',25,18,1,'Û',24,'Û  ',7
	db      '°',25,27,'°±²ÛÛÛ',25,7,'°±±²²',15,'Û',25,4,23,'°',7,16
	db      'Û',8,23,'°°±±²',24,1,16,'Û',26,24,'Ü Ü ÜÜ ÜÜ',25,3,'Ü'
	db      25,3,'Ü',25,15,8,23,'±°²Û²Û',16,'²±±° ',1,'ÜÛ Ü ÜÜÜß'
EndAnsi_Len     equ $-EndAnsi

ContactAnsi     label byte
	db      1,16,'Û',25,'MÛ',24,'Û',25,17,15,'-+ starplayer 2.25'
	db      's source code release +-',25,17,1,'Û',24,'Û',25,16,7
	db      'copyright (c) scott mcnab (jedi/oxygen) 1996',25,16,1
	db      'Û',24,'Û ',14,'contacting the author -',25,'5',1,'Û'
	db      24,'Û',25,'MÛ',24,'Û ',12,'email: ',7,'jedi@tartarus'
	db      '.uwa.edu.au',25,15,12,'snail: ',7,'scott mcnab',25,11
	db      1,'Û',24,'Û',25,7,7,'mcnab-sd@ee.uwa.edu.au',25,24,'5'
	db      ' honeydew cl',25,9,1,'Û',24,'Û',25,'6',7,'maida val'
	db      'e wa 6057',25,4,1,'Û',24,'Û',25,2,12,'www: ',7,'htt'
	db      'p://www.uwa.edu.au/student/jedi',25,12,'australia',25
	db      13,1,'Û',24,'Û',25,7,7,'http://www.it.com.au/oxygen/'
	db      25,')',1,'Û',24,'Û',26,3,'Ü ',26,5,'Ü ÜÜÜ ÜÜ Ü Ü',25,2
	db      'ÜÜ Ü Ü ',26,3,'Ü ',26,5,'Ü ',26,'!ÜÛ'
ContactAnsi_Len equ $-ContactAnsi

HeadLine        label byte
	db      1,16,'°',0,17,'±° ',15,'-+ starplayer +- ',3,'protec'
	db      'ted mode ',10,'multi-module ',3,'player  ',15,'(c) '
	db      'jedi / oxygen - ',0,'°±',1,16,'°',24
HeadLine_Len    equ $-HeadLine

HeaderAnsi      label byte
	db      1,23,' ',4,'memory ',0,'Ú ',4,'conventional',0,':',25
	db      13,4,'title',0,':',25,'$',24,25,7,'³ ',4,'extra',0,':'
	db      25,18,4,'pattern',0,':',25,9,4,'spd',0,':',25,5,4,'v'
	db      'olume',0,':',25,9,24,25,7,'À ',4,'soundcard',0,':',25
	db      18,4,'row',0,':',25,9,4,'bpm',0,':',25,7,4,'song',0,':'
	db      25,9
HeaderAnsi_Len  equ $-HeaderAnsi

CDStatusLine    label byte
	db      0,23,' ',4,'memory ',0,'Ú ',4,'conventional',0,':',25
	db      14,4,'ú cd audio mode ú',25,9,'volume',0,':',25,7,24,25
	db      7,'³ ',4,'extra',0,':',25,20,4,'track',0,':',25,8,4,'t'
	db      'ime',0,':',25,7,4,'remain',0,':',25,7,24,25,7,'À ',4
	db      'soundcard',0,':',25,15,4,'status',0,':',25,7,4,'tot'
	db      'al',0,':',25,7,4,'remain',0,':',25,7
CDStatusLine_Len equ $-CDStatusLine

InfoAnsi        label byte
	db      0,23,' Ú',26,5,'Ä ',4,'sample name ',0,26,5,'Ä¿  ',4,'#'
	db      '  vol. ',0,'ÚÄÄÄ ',4,'vu bar ',0,'ÄÄÄ¿ ',4,'pan ',0,'Ú'
	db      'ÄÄÄ ',4,'current fx ',0,'ÄÄÄ¿ '
InfoAnsi_Len    equ $-InfoAnsi

Menu_1          label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º  ',14,'slot',25,4,'úùú module  titles úùú',25,6,'s'
	db      'ize',25,3,'chan samps  len  patts  ',11,'º',25,2,24,25
	db      2,8,'º  ÚÄÄ',11,'¿ ',8,'Ú',26,27,'Ä',11,'¿ ',8,'Ú',26
	db      6,'Ä',11,'¿ ',8,'ÚÄÄ',11,'¿ ',8,'ÚÄÄÄ',11,'¿ ',8,'ÚÄ'
	db      'ÄÄ',11,'¿ ',8,'ÚÄÄÄ',11,'¿  º',25,2
Menu_1_Len      equ $-Menu_1

Menu_2          label byte
	db      0,19,25,2,8,'º  ³  ',11,'³ ',8,'³',16,25,27,11,19,'³'
	db      ' ',8,'³',25,6,11,'³ ',8,'³  ',11,'³ ',8,'³',25,2,11,'³'
	db      ' ',8,'³',25,2,11,'³ ',8,'³',25,2,11,'³  º',25,2
Menu_2_Len      equ $-Menu_2

Menu_3          label byte
	db      8,19,25,2,'º  À',11,'ÄÄÙ ',8,'À',11,26,27,'ÄÙ ',8,'À'
	db      11,26,6,'ÄÙ ',8,'À',11,'ÄÄÙ ',8,'À',11,'ÄÄÄÙ ',8,'À',11
	db      'ÄÄÄÙ ',8,'À',11,'ÄÄÄÙ  º',25,2,24,25,2,8,'È',11,26,'G'
	db      'Í¼',25,2,24,' Ú',26,'KÄ',8,'¿ ',24,' ',11,'³ L/TAB='
	db      'load C=cdmode P=play S=stop V=view F=free R=release'
	db      ' W=write ESC=quit ',8,'³ ',24,' ',11,'À',8,26,'KÄÙ '
	db      24,25,'O'
Menu_3_Len      equ $-Menu_3

Filemenu_1      label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º',25,6,'Ú',26,'7Ä',11,'¿',25,6,'º',25,2,24,25,2,8,'º'
	db      25,2,14,'path',8,'³',25,'7',11,'³',25,6,'º',25,2,24,25
	db      2,8,'º',25,6,'À',11,26,'7ÄÙ',25,6,'º',25,2,24,25,2,8,'º'
	db      25,6,14,'filename',25,5,'type',25,6,'úùú module  tit'
	db      'les úùú',25,7,'size',25,5,11,'º',25,2,24,25,2,8,'º',25
	db      2,'Ú',26,13,'Ä',11,'¿  ',8,'ÚÄÄÄ',11,'¿  ',8,'Ú',26,27
	db      'Ä',11,'¿  ',8,'Ú',26,6,'Ä',11,'¿',25,2,'º',25,2
Filemenu_1_Len  equ $-Filemenu_1

Filemenu_2      label byte
	db      0,19,25,2,8,'º',25,2,'³',16,25,13,11,19,'³  ',8,'³',25
	db      2,11,'³  ',8,'³',25,27,11,'³  ',8,'³',25,6,11,'³',25,2
	db      'º',25,2
Filemenu_2_Len  equ $-Filemenu_2

Filemenu_3      label byte
	db      0,19,25,2,8,'º',25,2,'À',11,26,13,'ÄÙ  ',8,'À',11,'Ä'
	db      'ÄÄÙ  ',8,'À',11,26,27,'ÄÙ  ',8,'À',11,26,6,'ÄÙ',25,2
	db      'º',25,2,24,25,2,8,'È',11,26,'GÍ¼',25,2,24,25,13,'Ú',26
	db      '1Ä',8,'¿',25,13,24,25,13,11,'³ SPACE=tag *=tag all '
	db      'ENTER=load/select ESC=cancel ',8,'³',25,13,24,25,13,11
	db      'À',8,26,'1ÄÙ',25,13,24,25,'O'
Filemenu_3_Len  equ $-Filemenu_3

FilePath        label byte
	db      8,23,25,'O',24,25,2,'É',26,'GÍ',15,'»',25,2,24,25,2,8
	db      'º',25,6,'Ú',26,'=Ä',15,'¿ º',25,2,24,25,2,8,'º ',4,'f'
	db      'ile: ',8,'³',25,'=',15,'³ º',25,2,24,25,2,8,'º',25,6
	db      'À',15,26,'=ÄÙ º',25,2,24,25,2,8,'È',15,26,'GÍ¼',25,2
	db      24,25,'O'
FilePath_Len    equ $-FilePath

View_1          label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º  ',14,'samp',25,9,'instrument name',25,10,'length'
	db      25,3,'startlp',25,2,'endloop',25,2,11,'º',25,2,24,25,2
	db      8,'º  ÚÄÄ',11,'¿  ',8,'Ú',26,28,'Ä',11,'¿  ',8,'Ú',26
	db      6,'Ä',11,'¿ ',8,'Ú',26,6,'Ä',11,'¿ ',8,'Ú',26,6,'Ä',11
	db      '¿  º',25,2
View_1_Len      equ $-View_1

View_2          label byte
	db      11,19,25,2,8,'º  ³  ',11,'³  ',8,'³',25,28,11,'³  ',8
	db      '³',25,6,11,'³ ',8,'³',25,6,11,'³ ',8,'³',25,6,11,'³'
	db      '  º',25,2
View_2_Len      equ $-View_2

View_3          label byte
	db      11,19,25,2,8,'º  À',11,'ÄÄÙ  ',8,'À',11,26,28,'ÄÙ  ',8
	db      'À',11,26,6,'ÄÙ ',8,'À',11,26,6,'ÄÙ ',8,'À',11,26,6,'Ä'
	db      'Ù  º',25,2,24,25,2,8,'È',11,26,'GÍ¼',25,2,24,25,23,'Ú'
	db      26,29,'Ä',8,'¿',25,23,24,25,23,11,'³ UP/DOWN=slide l'
	db      'ist  ESC=quit ',8,'³',25,23,24,25,23,11,'À',8,26,29,'Ä'
	db      'Ù',25,23,24,25,'O'
View_3_Len      equ $-View_3

HelpBar         label byte
	db      15,18,25,2,14,'F10',15,'=menu  ',14,'C',15,'=cd audi'
	db      'o  ',14,'D',15,'=dos shell  ',14,'Z',15,'=toggle vi'
	db      'deo  ',14,'?',15,'=help  ',14,'ESC',15,'=quit to do'
	db      's',25,2
HelpBar_Len     equ $-HelpBar

CD_Screen       label byte
	db      8,19,25,26,11,'Ú',26,22,'Ä',8,'¿',25,27,24,25,26,11,'³'
	db      25,2,14,'ú cd audio mode ú',25,2,8,'³',25,27,24,25,7,11
	db      'Ú',26,17,'ÄÙ',25,22,8,'À',11,26,17,'Ä',8,'¿',25,8,24
	db      25,7,11,'³',25,29,'³',25,29,8,'³',25,8,24,25,7,11,'³'
	db      25,5,14,'ESC ',11,'= quit menu',25,8,'³',25,7,14,'F1'
	db      '-F8 ',11,'= start track',25,2,8,'³',25,8,24,25,7,11,'³'
	db      25,29,'³',25,29,8,'³',25,8,24,25,7,11,'³',25,7,14,'C'
	db      ' ',11,'= quit cd audio',25,4,'³',25,5,14,'F11/F12 ',11
	db      '= back/forward  ',8,'³',25,8,24,25,7,11,'³',25,29,'³'
	db      25,29,8,'³',25,8,24,25,7,11,'³',25,6,14,'F9 ',11,'= '
	db      'stop/resume track ³ ',14,'ctrl-F11/F12 ',11,'= dec/'
	db      'inc track ',8,'³',25,8,24,25,7,11,'³',25,29,'³',25,29
	db      8,'³',25,8,24,25,7,11,'³',25,7,14,'E ',11,'= eject/c'
	db      'lose tray  ³  ',14,'alt-F11/F12 ',11,'= dec/inc vol'
	db      25,2,8,'³',25,8,24,25,7,11,'³',25,'<',8,'³',25,8,24,25
	db      7,11,'À',8,26,'<ÄÙ',25,8
CD_Screen_Len   equ $-CD_Screen

;----------------------------------------------------------------------------
; CODE
;----------------------------------------------------------------------------
_main:          sti

		movzx eax,[_filebuflen] ;First allocate file buffer
		call _getlomem
		jnc initfine
		jmp _exit
initfine:       mov [_filebufloc],eax

		push es                 ;Parse command line parameters
		push gs
		pop es
		mov edi,_pspa
		add edi,81h
		mov esi,offset Filename

@findspclp:     mov al,es:[edi]
		inc edi
		cmp al,13
		jz @nocmdline
		cmp al,32
		jz @findspclp
		cmp al,'-'
		jz @handleparm
		cmp al,'/'
		jz @handleparm

@@copylp:       mov ds:[esi],al
		inc esi
		mov al,es:[edi]
		inc edi
		cmp al,32
		jz @findspclp
		cmp al,13
		jz @nocmdline
		jmp @@copylp

@handleparm:    mov al,es:[edi]
		inc edi                 ;determine parameters
		cmp al,97
		jb @noajustp
		sub al,32
@noajustp:      cmp al,'?'
		jz @@showhelp
		cmp al,'C'
		jz @@selectcard
		cmp al,'H'
		jz @@showhelp
		cmp al,'M'
		jz @@getmixrate
		cmp al,'B'
		jz @@getbufsize
		cmp al,'D'              ;parameter '-d' = disable DMA
		jz @@disabledma
		cmp al,'Z'
		jz @@setscreen
		cmp al,'A'
		jz @@setamp
		jmp @findspclp
@@selectcard:   call ReadVariable
		cmp eax,1
		jb @@showhelp
		cmp eax,2
		ja @@showhelp
		mov [ForceCard],al

@@disabledma:   mov [__DMAFlag],0       ;disable DMA transfers (gus only)
		jmp @findspclp

@@getmixrate:   call ReadVariable       ;set command line mixing rate
		mov [Default_MixRate],ax
		jmp @findspclp
@@getbufsize:   call ReadVariable       ;set command line buffer size
		shr eax,4
		shl eax,4
		or eax,eax
		jnz @@validsize
		mov ax,1024
@@validsize:    mov [Default_BufLen],ax
		jmp @findspclp
@@showhelp:     pop es
		call ShowCmdLineHelp    ;show help and quit
		jmp _exit
@@setamp:       call ReadVariable       ;set amplification
		cmp eax,16
		ja @@nottoosmall
		mov eax,16
@@nottoosmall:  cmp eax,127
		jb @@nottoobig
		mov eax,127
@@nottoobig:    mov [Amplification],al
		jmp @findspclp

@@setscreen:    call ReadVariable       ;set screen size
		mov [Last_Row],al
		jmp @findspclp

@nocmdline:     pop es

		;get current DTA address
		mov [v86r_ah],02fh
		mov al,21h
		int 33h
		mov ax,[v86r_es]
		mov [DTASeg],ax
		mov ax,[v86r_bx]
		mov [DTAOff],ax

		mov dl,0
		mov eax,M_Struc_Size*MAXSLOTS
		call _gethimem          ;get hi-mem for song slot headers
		jc abortprog2
		push eax
		mov ecx,MAXSLOTS
		mov edi,offset Song_Table       ;set all slot pointers
@@slotloop:     stosd
		add eax,M_Struc_Size
		loop @@slotloop
		pop edi
		mov ecx,MAXSLOTS*M_Struc_Size   ;clear slots
		mov al,0
		rep stosb

		mov eax,160*50  ;160*(4+33) ;get hi-mem for temp screen buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer],eax

		mov eax,80*50*2         ;get hi-mem for menu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer2],eax

		mov eax,80*50*2         ;get hi-mem for filemenu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer3],eax

		mov eax,80*50*2         ;get hi-mem for another menu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer4],eax

		mov eax,EXTRADATASIZE   ;get hi-mem for temp menu stuff
		call _gethimem
		jc abortprog2
		mov [ExtraData],eax

		mov eax,FileDataSize*MAXLISTSIZE
		call _gethimem          ;get hi-mem for file-list
		jc abortprog2
		mov [FileList],eax

		mov eax,768             ;get mem for palette
		call _gethimem
		jc abortprog2
		mov [Temp_Palette],eax

		mov eax,LISTSIZEMAX             ;get hi-mem for list buffers
		mov [ListFileFree],eax
		call _gethimem
		jc abortprog2
		mov [ListFileBuffer],eax

@@nochange:     mov [v86r_ax],0003h            ;Use bios to set videomode
		mov al,10h
		int 33h

		call SetScreenSize

		call PrepareScreen

		call HookKeyboard
		mov [ProtectFlag],0

		mov al,[ForceCard]
		;mov al,DEVICE_AUTO
		mov bx,[Default_MixRate]
		mov dx,[Default_BufLen]
		call PM_InitSystem
		mov dl,1
		jc abortprog

		call CD_Installed               ;check for cdrom drive
		jnc @@nocdrom
		mov [CDROM_Flag],1
		call CD_Done_Play               ;check if cd is playing
		jz @@nocdrom
		inc [CDROM_Playing]
		inc [CDROM_Mode]
@@nocdrom:      call ShowDevice

		mov esi,offset Filename
		cmp byte ptr [esi],0
		jz @@activatemenu
		mov [stflag],1                  ;load and play first
		mov [ListFlag],0
		call Load_Slots
		jnc @@prenoshowit
		mov edx,offset Error_Msg3
		mov ax,0900h
		call OutMsg
		jmp short @@prenoshowit
@@activatemenu: inc [PopUpFlag]                 ;call menu if no filespec
		or byte ptr gs:[417h],00010000b ;Force scroll lock on
		call MenuCode
		dec [PopUpFlag]
@@prenoshowit:  mov [stflag],0

		call ParseInput

@@end:          call PM_CloseSystem
		call PrepareScreen

		call FreeKeyboard       ;normal exit
		call EndScreen
		jmp _exit

abortprog:      push edx
		call FreeKeyboard
		pop edx
abortprog2:     push edx
		call EndScreen
		pop edx
		mov eax,offset ErrorMsg         ;display error msgs
		call @writestr
		movzx eax,dl
		mov eax,[ErrorTable+eax*4]
		call @writestr
		jmp _exit

;----------------------------------------------------------------------------
SetScreenSize   proc near       ;set screen size

		mov [v86r_ax],00083h            ;Reset videomode
		mov al,10h
		int 33h
		mov [Scr_Ypos],24
		mov al,[Last_Row]
		cmp al,50
		jb @@donehere
		mov [v86r_ax],1112h             ;Set small fonts
		mov [v86r_bx],0
		mov al,10h
		int 33h
		mov [Scr_Ypos],49
		mov [Last_Row],50
		ret
@@donehere:     mov [Last_Row],25
		ret
SetScreenSize   endp

;----------------------------------------------------------------------------
ParseInput      proc near               ;handle input

@@waitloop:     call PrepareScreen

		@rlp edi,0b8000h        ;Draw help bar
		movzx eax,byte ptr gs:[0484h]
		mov ebx,160
		mul ebx
		add edi,eax
		mov esi,offset HelpBar
		mov ecx,HelpBar_Len
		call DeCrunch

		or byte ptr gs:[417h],00010000b ;Force scroll lock on

		mov [v86r_ax],0100h     ;First check for key
		mov al,16h
		int 33h
		jz @@waitloop

		@rlp edi,0b8000h        ;clear help bar
		movzx eax,byte ptr gs:[0484h]
		mov ebx,160
		mul ebx
		add edi,eax
		xor eax,eax
		mov ecx,160/4
		rep stosd

		mov [v86r_ax],0         ;Use real-mode int 16h for read char
		mov al,16h
		int 33h

		mov ax,[v86r_ax]
		cmp ah,1
		jz @@quit
		cmp ah,15       ;tab
		jz @@doload
		cmp ah,39h      ;al,32 ;space
		jz @@getmenu
		cmp ah,1ch      ;al,13
		jz @@getmenu
		cmp al,97
		jb @@nofix
		sub al,('a'-'A')
@@nofix:        cmp al,'L'
		jz @@doload
		cmp al,'D'
		jz @@dosshell
		cmp al,'Q'
		jz @@quit
		cmp al,'S'
		jz @@stop
		cmp al,'?'
		jz @@help
		cmp al,'Z'
		jz @@video
		cmp al,'C'
		jz @@cdmode

		jmp @@waitloop

@@doload:       call TabFile
		jc @@noload
		mov [ListFlag],0
		call Load_Slots
@@noload:       jmp @@waitloop

@@getmenu:      cmp [PopUpFlag],0       ;bring up menu
		jnz @@waitloop
		inc [PopUpFlag]
		call MenuCode           ;menu code in here
		dec [PopUpFlag]
		jmp @@waitloop

@@dosshell:     call Drop2Dos
		jmp @@waitloop

@@stop:         call PM_StopModule      ;stop module
		mov [CurrentSong],0
		jmp @@waitloop

@@help:         call ShowHelp           ;show hot-keys help
		jmp @@waitloop

@@video:        cmp [Last_Row],50       ;toggle screen size
		jnz @@make50
		mov [Last_Row],25
		jmp short @@donemake
@@make50:       mov [Last_Row],50
@@donemake:     call SetScreenSize
		jmp @@waitloop

@@cdmode:       cmp [CDROM_Mode],0      ;enable CD audio mode
		jz @@cdmodeon
		call StopCDMode
		jmp @@waitloop
@@cdmodeon:     inc [CDROM_Mode]
		call PM_StopModule
		mov [CurrentSong],0
		jmp @@waitloop

@@quit:         ret
ParseInput      endp

;----------------------------------------------------------------------------
ShowCmdLineHelp proc near
		mov eax,offset Init_Msg0
		call @writestr
		mov eax,offset CmdLne_Help1
		call @writestr
		ret
@writestr:      call ptr2segoff
		mov [v86r_ah],9
		mov [v86r_ds],bx
		mov [v86r_dx],ax
		mov al,21h
		int 33h
		ret

ShowCmdLineHelp endp

;----------------------------------------------------------------------------
ReadVariable    proc near               ;Read command line number and
					;return in eax
		push ebx ecx edx
		xor eax,eax
		xor edx,edx
		mov ebx,10
@@readnext:     mov cl,es:[edi]
		inc edi
		sub cl,'0'
		cmp cl,9
		ja @@abort
		movzx ecx,cl
		mul ebx
		add eax,ecx
		jmp @@readnext
@@abort:        dec edi
		pop edx ecx ebx
		ret
ReadVariable    endp

;----------------------------------------------------------------------------
HookKeyboard    proc near               ;Hook keyboard interrupt

		cli                     ;Hook irq 1
		mov bl,1
		call _getirqvect
		mov [Old_Key_Handler],edx
		mov bl,1
		mov edx,offset New_Key_Handler
		call _setirqvect
		mov bl,1
		mov edx,offset New_Key_Handler
		mov edi,offset _RealModeCode
		call _rmpmirqset
		mov [_OldRealMode],eax

		sti
		ret
HookKeyboard    endp

FreeKeyboard    proc near               ;Release keyboard interrupt
		cli
		mov edx,[Old_Key_Handler]       ;Restore IRQ handler
		mov bl,1
		call _setirqvect
		mov bl,1                        ;Restore real mode IRQ handler
		mov eax,[_OldRealMode]
		call _rmpmirqfree

		sti
		ret
FreeKeyboard    endp

New_Key_Handler proc near               ;Hooked keyboard handle code
		pushf
		pushad
		push ds es fs gs
		mov ax,cs:_seldata
		mov ds,ax
		mov es,ax
		mov fs,ax
		mov gs,cs:_selzero

		in al,60h               ;buffer key
		cmp al,7fh
		ja @@notpress
		mov [ScanCode],al
@@notpress:     mov ah,gs:[417h]
		test ah,00010000b       ;Only work if scroll-lock is on
		jz @@continue
		test ah,00001000b       ;check for ctrl-alt too
		jz @@notctralt
		test ah,00000100b
		jz @@notctralt
		in al,60h
		mov ah,al
		and al,07fh
		cmp al,53h              ;handle control-alt-delete
		jz @@ctrlaltdel
		jmp short @@gotkey
@@notctralt:    in al,60h
		mov ah,al
		and al,07fh
@@gotkey:       cmp al,044h
		jz @@F10
		cmp al,57h              ;Check for F11 (back pattern)
		jz @@F11
		cmp al,58h              ;Check for F12 (forward pattern)
		jz @@F12
		cmp al,03bh             ;Check F1-F9
		jb @@continue
		cmp al,043h
		ja @@continue
		and ah,80h              ;Check key press
		or ah,ah
		jnz @@trapped
		cmp al,43h
		jz @@stopmod            ;Check F9 (stop module)
		sub al,3ah
@@startsong:    cmp [ProtectFlag],0
		jnz @@trapped

@@startsong2:   mov ah,gs:[417h]        ;set loopflag depending on shift state
		and ah,00000011b
		jz @@nopeloop
		mov ah,0
		jmp short @@donesetlp
@@nopeloop:     mov ah,1
@@donesetlp:    push eax                ;acknowledge int first to enable irqs
		call @@clearport
		pop eax
		sti

		cmp [CDROM_Mode],0      ;CDROM: start song (F-key)
		jz @@notcd3
		call PlayCDSlot
		jmp @@complete

@@notcd3:       call Play_Slot
		jmp @@complete
@@trapped:      call @@clearport
		jmp @@complete

@@continue:     mov cx,code16
		mov dx,offset JumpCode
		int 32h                 ; call real mode far routine
@@complete:     pop gs fs es ds
		popad
		popf
		iret
@@stopmod:      cmp [CDROM_Mode],0      ;CDROM: stop playing
		jz @@notcd0
		cmp [CDROM_Playing],0
		jnz @@stopcd
		call CD_Resume_Audio    ;  resume playing cd
		inc [CDROM_Playing]
		jmp @@trapped

@@stopcd:       call CD_Stop_Audio
		;mov al,DOORUNLOCK
		;call CD_Lock
		mov [CDROM_Playing],0
		jmp @@trapped

@@notcd0:       call PM_StopModule      ;F9 = Stop current module
		mov [CurrentSong],0
		jmp @@trapped

@@F10:          cmp [ProtectFlag],0     ;check not protected
		jnz @@trapped
		and ah,80h              ;Handle F10
		or ah,ah                ;F10 = popup menu
		jnz @@trapped

		cmp [PopUpFlag],0
		jnz @@trapped
		inc [PopUpFlag]
		call @@clearport
		sti

		call MenuCode           ;menu code in here

		dec [PopUpFlag]
		jmp @@complete
@@F11:          and ah,80h              ;Handle F11
		or ah,ah
		jnz @@trapped
		mov al,gs:[417h]
		test al,00001000b       ;Check if ALT+F11
		jz @@normalf11

		cmp [CDROM_Mode],0      ;CDROM: decrease volume
		jz @@notcd1

		call DecCDVol
		jmp @@trapped

@@notcd1:       mov al,[PlayerVolume]
		or al,al
		jz @@trapped
		dec al
		mov [PlayerVolume],al
		call PM_SetMasterVol
		jmp @@trapped
@@normalf11:    test al,00000100b       ;check if CTRL+F11
		jz @@notctrlf11

		cmp [CDROM_Mode],0      ;CDROM: decrease current track
		jz @@notcd2b

		mov eax,offset CDPlayStatus
		call CD_GetPos
		mov al,[CDPlayStatus._track]    ;get current track
		call BCDToDec
		dec al
		cmp al,[CDROM_STATS._low_audio] ;check lower track limit
		jae @@songok2
		mov al,[CDROM_STATS._high_audio]
@@songok2:      jmp @@startsong2

@@notcd2b:      mov al,[CurrentSong]    ;check not zero song
		or al,al
		jz @@trapped
		dec al                  ;decrease current song
		jnz @@startsong
		mov al,[CurrentSlot]
		jmp @@startsong

@@notctrlf11:   cmp [CDROM_Mode],0      ;CDROM: rewind cd
		jz @@notcd4c

		call CD_Head_Position   ;  decrease by CDSTEPTIME seconds

		push eax                ;determine if shift-key pressed
		mov ah,gs:[417h]
		and ah,00000011b
		pop eax
		jz @@notshift1
		sub eax,CDSTEPTIME*75

@@notshift1:    sub eax,CDSTEPTIME*75
		jns @@notneg
		xor eax,eax
@@notneg:       mov ecx,[CDROM_STATS._endofdisk]
		mov ebx,eax
		call CD_Play_Audio

		jmp @@trapped

@@notcd4c:      mov ebx,[__CurrentModule] ;dec pattern
		mov ax,[ebx+_MCurrentPos]
		sub ax,2
		or ax,ax
		jge @@tisgood
		mov ax,000feh
@@tisgood:      mov [ebx+_MCurrentPos],ax
		mov [ebx+_MCurrentRow],64
		jmp @@trapped
@@F12:          and ah,80h              ;Handle F12
		or ah,ah
		jnz @@trapped
		mov al,gs:[417h]
		test al,00001000b       ;Check if ALT+F12
		jz @@normalf12

		cmp [CDROM_Mode],0      ;CDROM: increase volume
		jz @@notcd2
		call IncCDVol
		jmp @@trapped

@@notcd2:       mov al,[PlayerVolume]
		cmp al,64
		jz @@trapped
		inc al
		mov [PlayerVolume],al
		call PM_SetMasterVol
		jmp @@trapped
@@normalf12:    test al,00000100b       ;check if CTRL+F12
		jz @@notctrlf12

		cmp [CDROM_Mode],0      ;CDROM: decrease current track
		jz @@notcd3b

		mov eax,offset CDPlayStatus
		call CD_GetPos
		mov al,[CDPlayStatus._track]    ;get current track
		call BCDToDec
		inc al
		cmp al,[CDROM_STATS._high_audio] ;check upper track limit
		jbe @@songok
		mov al,[CDROM_STATS._low_audio]
@@songok:       jmp @@startsong2

@@notcd3b:      mov al,[CurrentSong]    ;check not zero song
		or al,al
		jz @@trapped
		inc al                  ;increase current song
		cmp al,[CurrentSlot]
		jbe @@startsong
		mov al,1
		jmp @@startsong

@@notctrlf12:   cmp [CDROM_Mode],0      ;CDROM: fastforward cd
		jz @@notcd3c

		call CD_Head_Position   ;  increase by CDSTEPTIME seconds

		push eax                ;determine if shift-key pressed
		mov ah,gs:[417h]
		and ah,00000011b
		pop eax
		jz @@notshift2
		add eax,CDSTEPTIME*75

@@notshift2:    add eax,CDSTEPTIME*75
		mov ecx,[CDROM_STATS._endofdisk]
		mov ebx,eax
		call CD_Play_Audio

		jmp @@trapped

@@notcd3c:      mov ebx,[__CurrentModule] ;inc pattern
		mov [ebx+_MCurrentRow],64
		jmp @@trapped
@@clearport:    in al,61h               ;reset keyboard
		or al,80h
		out 61h,al
		and al,7fh
		out 61h,al
		mov al,20h
		out 20h,al
		ret

@@ctrlaltdel:   ;trap control-alt-delete and quit starplayer
		call PM_CloseSystem
		call FreeKeyboard
		call EndScreen
		call @@clearport
		jmp _exit

New_Key_Handler endp

;----------------------------------------------------------------------------
DecCDTrack      proc near               ;start previous track
		mov ax,[CDROM_STATS._current_track]
		mov eax,offset CDPlayStatus     ;draw drive stats
		call CD_GetPos
		mov al,[CDPlayStatus._track]    ;get current track
		call BCDToDec
		dec al
		cmp al,[CDROM_STATS._low_audio] ;check lower track limit
		ja @@songok
		mov al,[CDROM_STATS._low_audio]
@@songok:       call PlayCDSlot

		ret
DecCDTrack      endp

IncCDTrack      proc near               ;start next track
		ret
IncCDTrack      endp

;----------------------------------------------------------------------------
PlayCDSlot      proc near               ;start song AL, single flag in AH
		call InvertPal
		cmp al,[CDROM_STATS._low_audio]
		jb @@ret
		cmp al,[CDROM_STATS._high_audio]
		ja @@ret

		push eax
		movzx eax,al
		call CD_Set_Track
		pop eax

		cmp [CDROM_STATS._track_type],DATA_TRACK
		jz @@ret

		mov ebx,[CDROM_STATS._track_position]
		cmp ah,0
		jz @@1track
@@endofdisk:    mov ecx,[CDROM_STATS._endofdisk]
		jmp @@gotend

@@1track:       mov ax,[CDROM_STATS._current_track]
		inc ax
		cmp al,[CDROM_STATS._high_audio]
		ja @@endofdisk
		call CD_Set_Track
		mov ecx,[CDROM_STATS._track_position]
		dec ax
		call CD_Set_Track

@@gotend:       ;ebx=startpos, ecx=endpos
		;mov al,DOORLOCK
		;call CD_Lock

		mov eax,[CDROM_STATS._track_position]
		call CD_Seek
		call CD_Play_Audio

		cmp [CDROM_Mode],0
		jnz @@ret
		inc [CDROM_Mode]
		cmp [CDROM_STATS._error],0
		js @@ret

		inc [CDROM_Playing]

@@ret:          call UnInvertPal
		ret
PlayCDSlot      endp

DecCDVol        proc near               ;decrease CD volume by 1
		mov eax,offset CDVolume
		call CD_Get_Volume
		mov al,[CDVolume._volume0]
		or al,al
		jnz @@ok
		ret
@@ok:           dec al
		mov [CDVolume._volume0],al
		mov [CDVolume._volume1],al
		mov [CDVolume._volume2],al
		mov [CDVolume._volume3],al
		mov eax,offset CDVolume
		call CD_Set_Volume
		ret
DecCDVol        endp

IncCDVol        proc near               ;increase CD volume by 1
		mov eax,offset CDVolume
		call CD_Get_Volume
		mov al,[CDVolume._volume0]
		cmp al,255
		jnz @@ok
		ret
@@ok:           inc al
		mov [CDVolume._volume0],al
		mov [CDVolume._volume1],al
		mov [CDVolume._volume2],al
		mov [CDVolume._volume3],al
		mov eax,offset CDVolume
		call CD_Set_Volume
		ret
IncCDVol        endp

;----------------------------------------------------------------------------
MenuCode        proc near               ;popup menu code

		mov [v86r_ah],3         ;preserve char pos
		mov al,10h
		int 33h
		push [v86r_dx]
		push [v86r_bx]

		mov al,byte ptr gs:[0484h]      ;get screen size
		mov ah,al
		sub al,13
		mov [Menu_Size],al

		mov [v86r_ah],2         ;pos char off screen
		add ah,2
		mov [v86r_dh],ah
		mov al,10h
		int 33h

		mov edi,[ScreenBuffer2] ;copy existing screen
		@rlp esi,0b8000h
		mov ecx,160*50/4
		rep movsd

@@menustart:    cmp [CDROM_Mode],0              ;check if cdrom mode
		jz @@notcdmode

		@rlp edi,0b8000h+160*4
		movzx ecx,byte ptr gs:[0484h]
		sub ecx,3
		mov eax,160/4
		mul ecx
		mov ecx,eax
		mov eax,030003000h
		rep stosd

		@rlp edi,0b8000h+160*5          ;Draw menu-ansi
		mov esi,offset CD_Screen
		mov ecx,CD_Screen_Len
		call DeCrunch
		jmp @@waitloop

@@notcdmode:    @rlp edi,0b8000h+160*4          ;Draw menu-ansi
		mov esi,offset Menu_1           ;draw top of menu
		mov ecx,Menu_1_Len
		call DeCrunch

		add edi,160*4
		movzx ecx,[Menu_Size]
@@nextline:     push ecx                        ;draw middle lines
		mov esi,offset Menu_2
		mov ecx,Menu_2_Len
		call DeCrunch
		pop ecx
		add edi,160
		loop @@nextline

		mov esi,offset Menu_3           ;draw bottom of menu
		mov ecx,Menu_3_Len
		call DeCrunch

@@waitloop:     call PrepareScreen

		cmp [CDROM_Mode],0
		jnz @@cdmode
		call ShowMenuInfo
@@cdmode:
		mov [v86r_ax],0100h     ;First check for key
		mov al,16h
		int 33h
		jz @@waitloop

		mov [v86r_ax],0         ;Use real-mode int 16h for read char
		mov al,16h
		int 33h
		mov ax,[v86r_ax]
		cmp ah,1                ;if ESC, quit
		jz @@donemenu

		cmp [CDROM_Mode],0      ;cdrom menu options
		jz @@notcdrom3
		cmp al,97
		jb @@nocapsajust2
		sub al,32
@@nocapsajust2: cmp al,'C'
		jz @@togglecd
		cmp al,'E'
		jz @@ejectcd
		jmp @@waitloop

@@notcdrom3:    cmp ah,48h              ;normal menu options
		jz @@uparrow
		cmp ah,50h
		jz @@dnarrow
		cmp ah,47h              ;home
		jz @@home
		cmp ah,4fh              ;end
		jz @@end
		cmp ah,49h              ;pageup
		jz @@pgup
		cmp ah,51h              ;pagedown
		jz @@pgdn

		cmp al,13               ;enter
		jz @@mplay2
		cmp al,32               ;space
		jz @@mplay1
		cmp ah,15               ;tab
		jz @@tabload
		cmp al,97
		jb @@nocapsajust
		sub al,32
@@nocapsajust:  cmp al,'P'
		jz @@mplay1
		cmp al,'S'
		jz @@mstop
		cmp al,'R'
		jz @@mrelease
		cmp al,'F'
		jz @@mfree
		cmp al,'L'
		jz @@mload
		cmp al,'W'
		jz @@write
		cmp al,'V'
		jz @@mview
		cmp al,'C'
		jz @@togglecd
		jmp @@waitloop

@@mplay:        movzx eax,[Menu_Highlight]      ;play module
		mov ah,gs:[417h]                ;set loopflag depending on shift state
		and ah,00000011b
		jz @@nopeloop
		mov ah,0
		jmp short @@donesetlp
@@nopeloop:     mov ah,1
@@donesetlp:    call Play_Slot
		ret

@@mplay1:       call @@mplay
		jmp @@waitloop

@@mplay2:       call @@mplay
		jmp @@donemenu

@@mstop:        call PM_StopModule              ;stop module
		mov [CurrentSong],0
		jmp @@waitloop

@@tabload:      call TabFile
		jc @@waitloop

		mov [v86r_ah],02fh      ;preserve DTA address
		mov al,21h
		int 33h
		push [v86r_es]          ;segment
		push [v86r_bx]          ;offset
		mov [v86r_ah],01ah      ;set DTA address to starplayer
		mov ax,[DTASeg]
		mov [v86r_ds],ax
		mov ax,[DTAOff]
		mov [v86r_dx],ax
		mov al,21h
		int 33h

		mov [ListFlag],1
		call Load_Slots

		mov [v86r_ah],01ah              ;set DTA address again
		pop [v86r_dx]           ;offset
		pop [v86r_ds]           ;segment
		mov al,21h
		int 33h
		jmp @@waitloop

@@write:        movzx eax,[Menu_Highlight]      ;write module to disk
		push eax
		call TabFile
		pop eax
		jc @@waitloop
		call WriteModule
		jmp @@waitloop

@@mrelease:     movzx eax,[CurrentSlot]         ;release last module
		dec eax
		mov ebx,[Song_Table+eax*4]
		cmp ebx,[__CurrentModule]
		jnz @@okness
		mov [CurrentSong],0
@@okness:       mov al,1
		call PM_ReleaseModule
		jc @@waitloop
		dec [CurrentSlot]
		mov al,[Menu_Highlight]
		cmp al,[CurrentSlot]
		jl @@nochange
		mov al,[CurrentSlot]
		or al,al
		jg @@nochange
		inc al
@@nochange:     mov [Menu_Highlight],al
		cmp [Menu_TopSlot],al
		jbe @@waitloop
		mov [Menu_TopSlot],al
		jmp @@waitloop
@@mfree:        call FreeDevRam
		jmp @@waitloop
@@mload:        call LoadFileMenu
		jmp @@waitloop

@@mview:        mov al,[Menu_Highlight]
		call MViewSamples               ;view samples
		jmp @@waitloop

@@home:         mov al,1                        ;home key
		jmp @@ajustup
@@end:          mov al,[CurrentSlot]            ;end key
		jmp @@ajustup
@@pgup:         mov al,[Menu_Highlight]
		sub al,[Menu_Size]              ;page up key
		cmp al,1
		jge @@ajustup
		mov al,1
		jmp @@ajustup
@@pgdn:         mov al,[Menu_Highlight]         ;page down key
		add al,[Menu_Size]
		cmp al,[CurrentSlot]
		jb @@ajustup
		mov al,[CurrentSlot]
		jmp @@ajustup

@@uparrow:      mov al,[Menu_Highlight]         ;move selector up 1 line
		cmp al,0
		jz @@waitloop
		dec al
		jnz @@ajustup
		inc al
@@ajustup:      cmp al,[Menu_TopSlot]
		jae @@noajustup1
		mov [Menu_TopSlot],al
@@noajustup1:   jmp short @@dnfine

@@dnarrow:      mov al,[Menu_Highlight]         ;move selector dn 1 line
		cmp al,0
		jz @@waitloop
		inc al
		cmp al,[CurrentSlot]
		jbe @@dnfine
		dec al
@@dnfine:       mov ah,[Menu_TopSlot]
		add ah,[Menu_Size]
		cmp al,ah
		jb @@dnok2go
		mov ah,al
		sub ah,[Menu_Size]
		inc ah
		mov [Menu_TopSlot],ah
@@dnok2go:      mov [Menu_Highlight],al
		jmp @@waitloop

@@togglecd:     cmp [CDROM_Mode],0              ;switch cd/mod mode
		jz @@startcd
		call StopCDMode
		jmp @@menustart
@@startcd:      call PM_StopModule
		mov [CurrentSong],0
		inc [CDROM_Mode]
		jmp @@menustart

@@ejectcd:      call CD_Status
		mov [CDROM_StatusFlag],eax

		and eax,CDSTAT_DOOROPEN         ;check if door open
		jz @@cdclosed1
		mov al,CLOSE_TRAY               ;close tray
		call CD_Cmd
		inc [CDWasOpenFlag]
		jmp @@waitloop

@@cdclosed1:    call CD_Stop_Audio
		mov [CDROM_Playing],0
		mov al,EJECT_TRAY               ;open tray
		call CD_Cmd
		jmp @@waitloop

@@donemenu:     mov esi,[ScreenBuffer2] ;replace existing screen
		@rlp edi,0b8000h
		mov ecx,160*50/4
		rep movsd

		pop [v86r_bx]
		pop [v86r_dx]
		mov [v86r_ah],2                 ;restore char pos
		mov al,10h
		int 33h
		ret
MenuCode        endp

ShowMenuInfo    proc near
		mov esi,offset Song_Table       ;draw info on screen
		@rlp edi,0b8000h+(160*8)+(7*2)
		mov cl,[Menu_Size]
		mov ch,[Menu_TopSlot]
		movzx eax,ch
		dec eax
		shl eax,2
		add esi,eax
@@nextslot:     push edi ecx
		push esi

		push edi ecx                    ;blank
		mov eax,3f003f00h
		stosd                           ;blank slot
		add edi,3*2
		xor eax,eax                     ;blank title block
		mov ecx,28
		rep stosw
		mov eax,3f003f00h
		add edi,(44-(13+28))*2
		mov ecx,7                       ;blank size
		rep stosw
		add edi,3*2
		stosd                           ;blank chan
		add edi,3*2
		stosd
		stosw                           ;blank samps
		add edi,3*2
		stosd
		stosw                           ;blank len
		add edi,3*2
		stosd
		stosw                           ;blank patts
		pop ecx edi

		cmp dword ptr [esi],0
		jz @@doneslot

		mov esi,[esi]                   ;copy title
		mov ebx,esi
		cmp [esi+_Pointer],0
		jz @@doneslot

		cmp ch,[CurrentSong]            ;highlight current song
		jnz @@notcurrents
		push edi ecx
		add edi,5*2+1
		mov al,040h
		mov ecx,28
@@hi1:          or [edi],al
		inc edi
		inc edi
		loop @@hi1
		pop ecx edi

@@notcurrents:  cmp ch,[Menu_Highlight]
		jnz @@nothilite
		push edi ecx                    ;highlight block
		add edi,5*2+1
		mov al,010h
		mov ecx,28
@@hi2:          or [edi],al
		inc edi
		inc edi
		loop @@hi2
		pop ecx edi

@@nothilite:    movzx eax,ch                    ;draw slot number
		call Bin2Dec
		mov ah,15+3*16
		cmp ch,10
		jae @@notlittle
		mov byte ptr [edi+2],0
@@notlittle:    push edi esi
		call @putdec2B
		pop esi edi

		add esi,_Title
		add edi,6*2
		push edi
		mov ecx,28
		mov ah,15                       ;colour 15 if loaded
		cmp [ebx+_SampleFlag],0
		jnz @@sloaded
		mov ah,7
@@sloaded:      lodsb
		or al,al
		jz @@donet
		stosb
		or [edi],ah     ;stosw
		inc edi
		loop @@sloaded
@@donet:        pop edi
		mov eax,[ebx+_Size]   ;draw size
		call Bin2Dec
		add edi,30*2
		mov ah,15+3*16
		push edi esi
		call @putdec2B
		pop esi edi

		movzx eax,[ebx+_TotalChanNum]   ;draw # chan
		call Bin2Dec
		add edi,10*2
		mov ah,15+3*16
		push edi esi
		call @putdec2B
		pop esi edi

		movzx eax,[ebx+_Insnum]         ;draw # samps
		call Bin2Dec
		add edi,5*2
		mov ah,15+3*16
		push edi esi
		call @putdec2B
		pop esi edi

		movzx eax,[ebx+_Ordnum]         ;draw # order
		call Bin2Dec
		add edi,6*2
		mov ah,15+3*16
		push edi esi
		call @putdec2B
		pop esi edi

		movzx eax,[ebx+_Patnum]         ;draw # patts
		call Bin2Dec
		add edi,6*2
		mov ah,15+3*16
		push edi esi
		call @putdec2B
		pop esi edi

@@doneslot:     pop esi
		pop ecx edi
		add esi,4
		add edi,160
		inc ch
		dec cl
		jnz @@nextslot

		ret
ShowMenuInfo    endp

;------------------------
MViewSamples    proc near               ;show samples in module AL
		or al,al
		jz @@nope
		movzx eax,al
		dec eax
		mov ebx,[Song_Table+eax*4]
		cmp dword ptr [ebx+_Size],0     ;check not zero length
		jnz @@valid
@@nope:         ret
@@valid:        inc [PopUpFlag]

		mov al,[Menu_TopSlot]           ;preserve slot pos
		push eax
		mov [Menu_TopSlot],1

		push ebx

		mov edi,[ScreenBuffer3]         ;copy existing screen
		@rlp esi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		;@rlp edi,0b8000h+160*4          ;Draw view-ansi
		mov edi,[ScreenBuffer4]
		mov esi,offset View_1           ;draw top of view menu
		mov ecx,View_1_Len
		call DeCrunch
		add edi,160*4
		movzx ecx,[Menu_Size]
@@nextline:     push ecx                        ;draw middle lines
		mov esi,offset View_2
		mov ecx,View_2_Len
		call DeCrunch
		pop ecx
		add edi,160
		loop @@nextline
		mov esi,offset View_3           ;draw bottom of view menu
		mov ecx,View_3_Len
		call DeCrunch

		pop ebp

@@waitloop:     push ebp
		call PrepareScreen
		pop ebp

		mov esi,[ScreenBuffer4]         ;draw buffered screen
		@rlp edi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		mov esi,[ebp+_Pointer]
		mov ebx,esi
		movzx ecx,[ebp+_Insnum]
		movzx eax,[ebp+_Ordnum]
		add ebx,60h             ;esi=start of module
		add ebx,eax             ;ebx=offset of sample table
		;@rlp edi,0b8000h+8*160
		mov edi,[ScreenBuffer4]
		add edi,4*160
		or ecx,ecx
		jz @@donelist

		movzx eax,[Menu_TopSlot]
		mov edx,eax
		dec eax
		sub ecx,eax
		add ebx,eax
		add ebx,eax

		movzx eax,[Menu_Size]
		cmp ecx,eax
		jle @@listloop
		mov ecx,eax

@@listloop:     push edx ecx

		push edi                        ;blank row
		add edi,7*2
		mov eax,03f003f00h
		stosd
		add edi,(14-9)*2
		mov ecx,28/2
		rep stosd
		add edi,8
		mov ecx,7
		rep stosw
		add edi,3*2
		mov ecx,7
		rep stosw
		add edi,3*2
		mov ecx,7
		rep stosw
		pop edi

		mov eax,edx                      ;slot number
		call Bin2Dec
		mov ah,3fh
		push edi esi
		add edi,7*2
		call @putdec2B
		pop esi edi

		movzx edx,word ptr [ebx]        ;draw instrument name
		add ebx,2
		shl edx,4
		add edx,esi
		mov ah,3fh
		push esi edi
		add edi,14*2
		mov esi,edx
		add esi,30h
@@nxt:          lodsb
		or al,al
		jz @@nomoretitle
		stosw
		jmp @@nxt
@@nomoretitle:  pop edi esi

		mov eax,[edx+10h]       ;put length
		or eax,eax
		jz @@nostats
		call Bin2Dec
		mov ah,3fh
		push edi esi
		add edi,47*2
		call @putdec2B
		pop esi edi

		mov eax,[edx+14h]       ;put loop start
		call Bin2Dec
		mov ah,3fh
		push edi esi
		add edi,57*2
		call @putdec2B
		pop esi edi

		mov eax,[edx+18h]       ;put loop end
		call Bin2Dec
		mov ah,3fh
		push edi esi
		add edi,67*2
		call @putdec2B
		pop esi edi

@@nostats:      add edi,160
		pop ecx edx
		inc edx
		dec ecx
		jnz @@listloop

@@donelist:
		mov [v86r_ax],0100h     ;First check for key
		mov al,16h
		int 33h
		jz @@waitloop

		mov [v86r_ax],0         ;Use real-mode int 16h for read char
		mov al,16h
		int 33h
		mov ax,[v86r_ax]
		cmp ah,1                ;if ESC, quit
		jz @@donemenu
		cmp ah,48h
		jz @@up
		cmp ah,50h
		jz @@dn
		jmp @@waitloop

@@up:           mov al,[Menu_TopSlot]   ;up arrow
		cmp al,1
		jle @@waitloop
		dec al
		mov [Menu_TopSlot],al
		jmp @@waitloop
@@dn:           movzx eax,[Menu_TopSlot]        ;dn arrow
		add al,[Menu_Size]
		cmp ax,[ebp+_Insnum]
		jg @@waitloop
		inc al
		sub al,[Menu_Size]
		mov [Menu_TopSlot],al
		jmp @@waitloop

@@donemenu:     mov esi,[ScreenBuffer3]         ;replace existing screen
		@rlp edi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		pop eax
		mov [Menu_TopSlot],al

		dec [PopUpFlag]
		ret
MViewSamples    endp

;------------------------
TabFile         proc near
		inc [PopUpFlag]
		mov [v86r_ah],02fh      ;preserve DTA address
		mov al,21h
		int 33h
		push [v86r_es]          ;segment
		push [v86r_bx]          ;offset

		mov [v86r_ah],01ah      ;set DTA address to starplayer
		mov ax,[DTASeg]
		mov [v86r_ds],ax
		mov ax,[DTAOff]
		mov [v86r_dx],ax
		mov al,21h
		int 33h

		mov [v86r_ah],3         ;preserve char pos
		mov al,10h
		int 33h
		push [v86r_dx]
		push [v86r_bx]

		mov edi,[ScreenBuffer3]         ;copy existing screen
		@rlp esi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		@rlp edi,0b8000h;+160*4          ;Draw ansi
		mov eax,160
		movzx ebx,byte ptr gs:[0484h]
		sub bl,6
		mul ebx
		add edi,eax

		mov esi,offset FilePath
		mov ecx,FilePath_Len
		call DeCrunch

		mov edi,offset Filename
		mov ecx,FILENAMESIZE
		xor eax,eax
		rep stosb
		mov edi,offset Filename
		xor ecx,ecx

@@promptloop:   push edi ecx
		@rlp edi,0b8000h+13*2 ;+160*7+13*2     ;copy filename text
		mov eax,160
		movzx ebx,byte ptr gs:[0484h]
		sub bl,3
		mul ebx
		add edi,eax

		mov esi,offset Filename
		mov ah,071h
@@next:         lodsb
		stosw
		or al,al
		jnz @@next

		call PrepareScreen
		pop ecx edi

		mov [v86r_ax],0200h     ;Set cursorpos
		mov al,byte ptr gs:[0484h]
		sub al,3
		mov [v86r_dh],al
		mov al,cl
		add al,13
		mov [v86r_dl],al
		mov [v86r_bx],0
		mov al,10h
		int 33h

		mov [v86r_ax],0100h     ;First check for key
		mov al,16h
		int 33h
		jz @@promptloop
		mov [v86r_ax],0         ;Use real-mode int 16h for read char
		mov al,16h
		int 33h
		mov ax,[v86r_ax]
		cmp ah,1                ;if ESC, quit
		jz @@doneprompt
		cmp al,13
		jz @@loadfile

		cmp ah,0eh
		jz @@delete

		cmp al,33               ;check chars withing range
		jb @@promptloop
		cmp cl,60
		jae @@promptloop
		mov [edi],al
		inc edi
		inc ecx
		jmp @@promptloop

@@delete:       mov byte ptr [edi],0            ;handle backspace
		or ecx,ecx
		jz @@promptloop
		dec ecx
		dec edi
		mov byte ptr [edi],0
		jmp @@promptloop

@@loadfile:     mov byte ptr [edi],0
		xor eax,eax

@@doneprompt:   mov esi,[ScreenBuffer3]         ;replace existing screen
		@rlp edi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		pop [v86r_bx]
		pop [v86r_dx]
		mov [v86r_ah],2                 ;restore char pos
		mov al,10h
		int 33h

		mov [v86r_ah],01ah              ;set DTA address again
		pop [v86r_dx]                   ;offset
		pop [v86r_ds]                   ;segment
		mov al,21h
		int 33h
		dec [PopUpFlag]

		cmp ah,0                ;set carry flag appropriately
		jnz @@carry
		clc
		ret
@@carry:        stc
		ret
TabFile         endp

;----------------------------------------------------------------------------
LoadFileMenu    proc near               ;file menu code

		mov [v86r_ah],02fh      ;preserve DTA address
		mov al,21h
		int 33h
		push [v86r_es]          ;segment
		push [v86r_bx]          ;offset

		mov [v86r_ah],01ah      ;set DTA address to starplayer
		mov ax,[DTASeg]
		mov [v86r_ds],ax
		mov ax,[DTAOff]
		mov [v86r_dx],ax
		mov al,21h
		int 33h

		mov [FUpdateFlag],1
		movzx eax,[Menu_Size]
		push eax
		sub [Menu_Size],3

		mov edi,[ScreenBuffer3]         ;copy existing screen
		@rlp esi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd

		mov edi,[ScreenBuffer4]
		xor eax,eax
		mov eax,160*(50-4)/4
		rep stosd

		;@rlp edi,0b8000h+160*4          ;Draw menu-ansi
		mov edi,[ScreenBuffer4]
		mov esi,offset Filemenu_1       ;draw top of menu
		mov ecx,Filemenu_1_Len
		call DeCrunch

		add edi,160*7
		movzx ecx,[Menu_Size]
@@nextline:     push ecx                        ;draw middle lines
		mov esi,offset Filemenu_2
		mov ecx,Filemenu_2_Len
		call DeCrunch
		pop ecx
		add edi,160
		loop @@nextline

		mov esi,offset Filemenu_3       ;draw bottom of menu
		mov ecx,Filemenu_3_Len
		call DeCrunch

		call PreserveDir                ;preserve initial dir

@@waitloop:     cmp [FUpdateFlag],0
		jz @@noupdate
		call UpdateFilelist

@@noupdate:     call PrepareScreen

		@rlp edi,0b8000h+160*4          ;copy buffered screen
		mov esi,[ScreenBuffer4]
		mov ecx,160*(50-4)/4
		rep movsd

		;display file menu info

		;@rlp edi,0b8000h+160*7+13*2
		mov edi,[ScreenBuffer4]
		add edi,160*3+13*2

		mov esi,offset PreDirectory
		mov ah,03fh                     ;draw path
		mov ecx,54
@@nextpath:     lodsb
		or al,al
		jz @@donepath
		stosw
		loop @@nextpath
@@donepath:     or ecx,ecx                      ;  clear rest of block
		jz @@noblank
		mov ax,3f00h
		rep stosw
@@noblank:      mov esi,[FileList]              ;draw filenames
		;@rlp edi,0b8000h+(160*11)+(8*2)
		mov edi,[ScreenBuffer4]
		add edi,(160*7)+(8*2)

		mov cl,[Menu_Size]
		mov ch,[FMenu_TopSlot]
		movzx eax,ch
		dec eax
		mov ebx,FileDataSize
		mul ebx
		add esi,eax

@@nextname:     push ecx edi esi
		push edi                ;blank regions
		xor eax,eax
		mov ecx,14              ;filename
		rep stosw
		mov eax,03f003f00h      ;type
		add edi,4*2
		stosd
		stosw
		add edi,4*2             ;title
		mov ecx,28/2
		rep stosd
		add edi,4*2             ;size
		stosd
		stosd
		stosd
		stosw
		pop edi

		mov al,[esi]            ;check if file
		or al,al
		jz @@nofile

		mov ah,7                ;copy filename
		push esi edi
		test al,30h
		jz @@finecol
		mov ah,15
@@finecol:      inc edi
		inc edi
		inc esi
		mov ecx,13
@@nextfl:       lodsb
		or al,al
		jz @@doneit
		stosw
		loop @@nextfl
@@doneit:       pop edi esi

		test byte ptr [esi],30h
		jnz @@nofile

		push edi esi            ;draw file type
		movzx eax,byte ptr [esi]
		and al,7fh
		dec eax
		shl eax,2
		add eax,offset TypeTable
		mov esi,eax
		add edi,(26-8)*2
		mov ah,03fh
		mov cl,3
@@here:         lodsb
		stosw
		dec cl
		jnz @@here
		pop esi edi

		push edi esi            ;draw module title
		add edi,(33-8)*2
		add esi,_FileTitle
		mov ecx,28
		mov ah,03fh
@@titleloop:    lodsb
		stosw
		or al,al
		jz @@donetitle
		loop @@titleloop

@@donetitle:    pop esi edi

		mov eax,[esi+_FileSize]
		call Bin2Dec
		mov ah,3fh
		push edi esi
		add edi,(65-8)*2
		call @putdec2B
		pop esi edi

@@nofile:       pop esi edi ecx

		mov ah,0                ;determine hi-lite colour
		test byte ptr [esi],80h
		jz @@notmarked
		or ah,040h
@@notmarked:    cmp ch,[FMenu_Highlight]
		jnz @@nohilite
		or ah,010h
@@nohilite:     push edi                ;highlight select bar
		inc edi
		mov al,14
@@loophere:     or byte ptr [edi],ah
		inc edi
		inc edi
		dec al
		jnz @@loophere
		pop edi

		add edi,160
		add esi,FileDataSize
		inc ch
		dec cl
		jnz @@nextname
@@donelistfiles:
		mov [v86r_ax],0100h     ;First check for key
		mov al,16h
		int 33h
		jz @@waitloop
		mov [v86r_ax],0         ;Use real-mode int 16h for read char
		mov al,16h
		int 33h
		mov ax,[v86r_ax]
		cmp ah,1                ;if ESC, quit
		jz @@donemenu
		cmp ah,48h
		jz @@uparrow
		cmp ah,50h
		jz @@dnarrow
		cmp ah,47h              ;home
		jz @@home
		cmp ah,4fh              ;end
		jz @@end
		cmp ah,49h              ;pageup
		jz @@pgup
		cmp ah,51h              ;pagedown
		jz @@pgdn
		cmp al,32
		jz @@space
		cmp al,'*'
		jz @@asterix
		cmp al,13
		jz @@loadfiles
		jmp @@waitloop

@@space:        movzx eax,[FMenu_Highlight]
		dec eax
		mov ebx,FileDataSize
		mul ebx
		add eax,[FileList]
		test byte ptr [eax],16
		jnz @@waitloop
		test byte ptr [eax],32
		jnz @@waitloop
		xor byte ptr [eax],80h
		jmp @@waitloop

@@asterix:      movzx ecx,[LastFilled]          ;invert all file tags
		mov esi,[FileList]
@@starall:      test byte ptr [esi],16
		jnz @@notag
		test byte ptr [esi],32
		jnz @@notag
		xor byte ptr [esi],80h
@@notag:        add esi,FileDataSize
		loop @@starall
		jmp @@waitloop

;-----------
@@loadfiles:    mov esi,[FileList]              ;Load all tagged files
		movzx ecx,[LastFilled]

@@nextload:     push esi ecx

		test byte ptr [esi],16
		jnz @@noload
		test byte ptr [esi],32
		jnz @@noload

		mov al,[LastFilled]             ;check if highlighted
		sub al,cl
		inc al
		cmp al,[FMenu_Highlight]
		jz @@loadit
		cmp byte ptr [esi],80h
		jb @@noload                     ;check if actual file

@@loadit:       mov edi,offset SeekFilename
		inc esi
		mov ah,13
@@nextchar:     movsb
		dec ah
		jnz @@nextchar

		mov [ListFlag],1
		mov ah,1
		call Load_Slot

@@noload:       pop ecx esi
		add esi,FileDataSize
		dec ecx
		jnz @@nextload

		movzx eax,[FMenu_Highlight]     ;check if directory
		mov ebx,FileDataSize
		dec eax
		mul ebx
		add eax,[FileList]

		test byte ptr [eax],32          ;change drive
		jz @@notdrive
		inc eax
		mov al,[eax]
		sub al,'A'
		mov [v86r_dl],al        ;select drive
		mov [v86r_ah],0eh
		mov [v86r_al],0eh
		mov al,21h
		int 33h
		mov [FUpdateFlag],1
		jmp @@waitloop

@@notdrive:     test byte ptr [eax],16
		jz @@donemenu
		inc eax                         ;change directory
		mov esi,eax
		mov edi,offset FileFound
		mov ecx,13
@@go:           lodsb
		stosb
		or al,al
		jz @@donedir
		loop @@go
@@donedir:      mov edx,offset FileFound
		add edx,[_code32a]
		mov eax,edx
		shr edx,4
		and ax,0fh
		mov [v86r_dx],ax
		mov [v86r_ds],dx
		mov [v86r_ah],3bh
		mov al,21h
		int 33h

		mov [FUpdateFlag],1
		jmp @@waitloop

;-----------
@@home:         mov al,[FMenu_Highlight]         ;home key
		cmp al,0
		jz @@waitloop
		mov al,1
		mov [FMenu_Highlight],al
		jmp @@ajustup
@@end:          mov al,[FMenu_Highlight]         ;end key
		cmp al,0
		jz @@waitloop
		mov al,[LastFilled]
		jmp @@ajustup
@@pgup:         mov al,[FMenu_Highlight]         ;page up key
		cmp al,0
		jz @@waitloop
		sub al,[Menu_Size]
		cmp al,1
		jge @@ajustup
		mov al,1
		jmp @@ajustup
@@pgdn:         mov al,[FMenu_Highlight]         ;page down key
		cmp al,0
		jz @@waitloop
		add al,[Menu_Size]
		cmp al,[LastFilled]
		jb @@ajustup
		mov al,[LastFilled]
		jmp @@ajustup

@@uparrow:      mov al,[FMenu_Highlight]        ;move selector up 1 line
		cmp al,0
		jz @@waitloop
		dec al
		jnz @@ajustup
		inc al
@@ajustup:      cmp al,[FMenu_TopSlot]
		jae @@noajustup1
		mov [FMenu_TopSlot],al
@@noajustup1:   jmp short @@dnfine

@@dnarrow:      mov al,[FMenu_Highlight]        ;move selector dn 1 line
		cmp al,0
		jz @@waitloop
		inc al
		cmp al,[LastFilled]
		jbe @@dnfine
		dec al
@@dnfine:       mov ah,[FMenu_TopSlot]
		add ah,[Menu_Size]
		cmp al,ah
		jb @@dnok2go
		mov ah,al
		sub ah,[Menu_Size]
		inc ah
		mov [FMenu_TopSlot],ah
@@dnok2go:      mov [FMenu_Highlight],al
		jmp @@waitloop

@@donemenu:     call RestoreDir

		mov esi,[ScreenBuffer3]         ;replace existing screen
		@rlp edi,0b8000h+4*160
		mov ecx,160*(50-4)/4
		rep movsd
		pop eax
		mov [Menu_Size],al

		mov [v86r_ah],01ah              ;set DTA address again
		pop [v86r_dx]           ;offset
		pop [v86r_ds]           ;segment
		mov al,21h
		int 33h
		ret
LoadFileMenu    endp

;---------------
UpdateFilelist  proc near               ;update dir & file info

		call GetDirectory

		mov [LastFilled],0

		mov edi,[FileList]              ;blank list
		mov ecx,MAXLISTSIZE*FileDataSize
		mov al,0
		rep stosb

		mov al,04eh
		mov edi,[FileList]      ;loop to read all files in dir
@@filefinder:   push edi
		mov edx,offset FileFound
		mov edi,offset SeekPath
		mov cx,10h
		call FindFile
		pop edi
		jc @@nomorefiles

		mov [edi+_FileSize],ecx
		mov byte ptr [edi],16   ;get file type
		cmp bl,10h
		jz @@gotype             ;check if directory

		push edi
		add edi,_FileTitle
		push edx
		call PM_GetFileType
		pop edx
		pop edi
		jc @@nextfile
		mov byte ptr [edi],al

@@gotype:       mov esi,edx             ;copy filename
		push edi
		inc edi
		mov ecx,13
		rep movsb
		pop edi

		add edi,FileDataSize    ;increase pointers and loop
		inc [LastFilled]
@@nextfile:     cmp [LastFilled],MAXLISTSIZE
		jz @@nomorefiles2
		mov al,04fh
		jmp @@filefinder

@@nomorefiles:  push edi                ;sort files here
		mov edi,[FileList]
		movzx ecx,[LastFilled]
		push edi ecx
		call SortFilenames
		pop ecx edi
		call SortDirectories
		pop edi
		movzx ecx,[LastDrive]   ;loop to add drives to list
		mov eax,00003a41h       ;'A:\',0
@@drvloop:      push edi
		mov byte ptr [edi],32
		inc edi
		stosd
		pop edi
		add edi,FileDataSize    ;increase pointers and loop
		inc al
		inc [LastFilled]
		cmp [LastFilled],MAXLISTSIZE
		jz @@nomorefiles2
		loop @@drvloop

@@nomorefiles2: mov [FUpdateFlag],0
		mov [FMenu_TopSlot],1
		mov [FMenu_Highlight],1
		ret

UpdateFilelist  endp

SortFilenames   proc near               ;sort all filenames into alpha order
		dec ecx
@@mainloop:     push ecx edi

@@nextfile:     push ecx edi
		inc edi
		mov cl,12              ;check every letter
@@sorthere:     mov al,[edi]
		mov ah,[edi+FileDataSize]

		cmp al,0
		jz @@movedfile
		cmp al,'.'
		jz @@movedfile
		cmp ah,0
		jz @@swapfiles
		cmp ah,'.'
		jz @@swapfiles

		cmp al,ah
		ja @@swapfiles
		cmp al,ah
		jnz @@movedfile
		inc edi
		dec cl
		jnz @@sorthere
		jmp short @@movedfile

@@swapfiles:    pop esi
		push esi
		call SwitchFiles

@@movedfile:    pop edi ecx
		add edi,FileDataSize
		dec ecx
		jnz @@nextfile

		pop edi ecx             ;loop sort again
		dec ecx
		jnz @@mainloop
		ret
SortFilenames   endp

SwitchFiles     proc near               ;flip files at esi and esi+1
		push esi                ;1st -> temp
		mov edi,offset Filename
		mov ecx,FileDataSize
		rep movsb
		pop edi                 ;2nd -> 1st
		mov ecx,FileDataSize
		rep movsb
		mov esi,offset Filename ;temp -> 2nd
		mov ecx,FileDataSize
		rep movsb
		ret
SwitchFiles     endp

SortDirectories proc near               ;sort directories to front
		dec ecx
@@nextdir:      push edi ecx
@@nextloop:     test byte ptr [edi],16
		jnz @@noswap
		test byte ptr [edi+FileDataSize],16
		jz @@noswap
		push ecx edi
		mov esi,edi
		call SwitchFiles
		pop edi ecx
@@noswap:       add edi,FileDataSize
		dec ecx
		jnz @@nextloop
		pop ecx edi
		dec ecx
		jnz @@nextdir
		ret
SortDirectories endp

;---------------
GetDirectory    proc near               ;get current drive and dir into PreDirectory
		mov edi,offset Directory        ;clear directory string
		mov ecx,64/4
		xor eax,eax
		rep stosd

		mov [v86r_ah],19h       ;get current drive
		mov al,21h
		int 33h
		mov al,[v86r_al]
		add al,'A'
		mov [PreDirectory],al

		mov eax,offset Directory
		call ptr2segoff         ;get current directory
		mov [v86r_ds],bx
		mov [v86r_si],ax
		mov [v86r_dl],0
		mov [v86r_ah],47h
		mov al,21h
		int 33h
		ret
GetDirectory    endp

;---------------
PreserveDir     proc near               ;preserve dir & drive
		call GetDirectory
		mov edi,[ExtraData]
		mov esi,offset PreDirectory
		mov ecx,64+3
		rep movsb

		mov esi,offset PreDirectory
		mov al,[esi]
		sub al,'A'
		mov [v86r_dl],al        ;select drive
		mov [v86r_ah],0eh
		mov [v86r_al],0eh
		mov al,21h
		int 33h
		mov al,[v86r_al]
		mov [LastDrive],al      ;get last drive number
		ret
PreserveDir     endp
;---------------
RestoreDir      proc near               ;restore dir & drive
		mov esi,[ExtraData]
		push esi
		mov edi,offset PreDirectory
		mov ecx,64+3
		rep movsb

		pop esi
		mov al,[esi]
		sub al,'A'
		mov [v86r_dl],al        ;select drive
		mov [v86r_ah],0eh
		mov [v86r_al],0eh
		mov al,21h
		int 33h

		mov [v86r_ah],3bh
		mov eax,offset PreDirectory
		call ptr2segoff         ;set current directory
		mov [v86r_ds],bx
		mov [v86r_dx],ax
		mov al,21h
		int 33h
		ret
RestoreDir      endp

;----------------------------------------------------------------------------
FindFile        proc near               ;modified _findfile
		push eax                ;also returns:
		push esi                ;    BL=attribute
		push edi                ;    ECX=filesize
		add edi,[_code32a]
		mov esi,edi
		and esi,0fh
		shr edi,4
		mov [v86r_ds],di
		mov [v86r_dx],si
		mov [v86r_ah],al
		mov [v86r_cx],cx
		mov esi,[_code16a]
		sub esi,62h
		mov edi,edx
		mov al,21h
		int 33h
		mov ax,gs
		mov ds,ax
		mov bl,[esi-9]
		mov ecx,[esi-4]
		movsd
		movsd
		movsd
		movsb
		mov ax,es
		mov ds,ax
		pop edi
		pop esi
		pop eax
		ret
FindFile        endp

;----------------------------------------------------------------------------
Drop2Dos        proc near               ;Initiate a shell to dos
					; execute command EDX if <> 0
		;pushad
		mov [v86r_ax],4a00h             ;Deallocate mem
		mov eax,_pspa
		shr eax,4
		mov [v86r_es],ax

		;Resize dos ram for shell
		@php eax,[_lomembase]
		sub eax,[_pspa]
		shr eax,4
		inc eax

		;add eax,2048/16

		mov [v86r_bx],ax        ;128*1024/16       ;ax
		mov al,21h
		int 33h

		;Set up parameter block
		mov edi,_pspa
		movzx eax,word ptr gs:[edi+2ch]
		mov [ParaBlock],ax

		;Get COMSPEC=
		push es
		push gs
		pop es
		shl eax,4
		mov edi,eax
		mov edx,offset Comspec
checkvar:       mov cl,ComspecLen
		mov ebx,edx
scanvar:        mov al,byte ptr es:[edi]
		cmp al,byte ptr [ebx]
		jne skipvar
		inc edi
		inc ebx
		dec cl
		jnz scanvar

		;Now copy environment
		mov esi,offset Filename
@@copyloop:     mov al,es:[edi]
		inc edi
		mov ds:[esi],al
		inc esi
		or al,al
		jnz @@copyloop
		clc
		jmp complete
skipvar:        inc edi
		mov al,es:[edi]
		or al,al
		jnz skipvar
		inc edi
		mov al,es:[edi]
		or al,al
		jnz checkvar
		stc
complete:       pop es
		jc @@abort

		mov eax,offset ParamString
		call ptr2segoff
		mov [ParaBlock+2],ax
		mov [ParaBlock+4],bx

		mov [v86r_ax],00003h            ;Reset videomode
		mov al,10h
		int 33h
		call DrawStarAnsi
		mov [v86r_ax],0200h             ;Set cursor pos
		mov [v86r_dx],0a00h
		mov [v86r_bx],0
		mov al,10h
		int 33h
		mov eax,offset ExitString       ;Write exit string
		call ptr2segoff
		mov [v86r_ds],bx
		mov [v86r_dx],ax
		mov [v86r_ax],0900h
		mov al,21h
		int 33h

		mov eax,offset Filename          ;Set regs for shell
		call ptr2segoff
		mov [v86r_ds],bx
		mov [v86r_dx],ax
		mov [v86r_ax],04b00h
		mov eax,offset ParaBlock
		call ptr2segoff

		and byte ptr gs:[417h],11101111b ;Force scroll lock off

		mov [v86r_es],bx                ;and do the shell
		mov [v86r_bx],ax
		mov al,21h
		int 33h

		or byte ptr gs:[417h],00010000b ;Force scroll lock on

		mov [v86r_ax],04800h            ;Re-allocate ram
		mov [v86r_bx],0ffffh
		mov al,21h
		int 33h

		mov [v86r_ax],04a00h            ;do the re-allocation
		mov eax,_pspa
		shr eax,4
		mov [v86r_es],ax
		mov al,21h
		int 33h

		cmp [Last_Row],50
		jnz @@nocopy
		@rlp edi,0b8000h+160*25         ;copy screen so still visible
		@rlp esi,0b8000h
		mov ecx,160*25/4
		rep movsd
		@rlp edi,0b8000h
		xor eax,eax
		mov ecx,160*25/4
		rep stosd

@@nocopy:       call SetScreenSize

		mov [v86r_ah],01ah              ;set DTA address again
		mov ax,[DTASeg]
		mov [v86r_ds],ax
		mov ax,[DTAOff]
		mov [v86r_dx],ax
		mov al,21h
		int 33h

@@abort:        ;popad
		ret
ptr2segoff:     ;get PM pointer in EAX and return seg:off in BX:AX
		add eax,_code32a
		shld ebx,eax,28
		and eax,0fh
		ret
Drop2Dos        endp

;----------------------------------------------------------------------------
WriteModule     proc near               ;Write song number AL to disk
		inc [ProtectFlag]
		push eax

		mov edi,offset Filename         ;append .s3m extension
@@seeklp:       mov al,[edi]
		inc edi
		cmp al,0
		jz @@append
		cmp al,'.'
		jz @@noappend
		jmp short @@seeklp
@@append:       mov byte ptr [edi-1],'.'        ;copy extension
		mov byte ptr [edi],'S'
		mov byte ptr [edi+1],'3'
		mov byte ptr [edi+2],'M'
		mov byte ptr [edi+3],0

@@noappend:     ;pop ax edi
		pop eax

		movzx eax,al                    ;get ptr to module
		dec eax
		mov ebx,[Song_Table+eax*4]
		cmp dword ptr [ebx+_Size],0     ;check not zero length
		jz @@abort

		mov edx,offset Filename
		call _openfile                  ;check if exists
		jnc @@abortc
		call _createfile                ;if not, create, else error
		jc @@abort
		mov ecx,[ebx+_Size]
		mov edx,[ebx+_Pointer]          ;ebx=pointer to module in ram
		call _writefile                 ;write file
		jc @@abortc
		call _closefile                 ;close file
		dec [ProtectFlag]
		clc
		ret
@@abortc:       call _closefile
@@abort:        dec [ProtectFlag]
		stc
		ret
WriteModule     endp

;----------------------------------------------------------------------------
FreeDevRam      proc near
		movzx eax,[CurrentSlot]         ;Free last module sample ram
@@freeloop:     dec eax
		mov ebx,[Song_Table+eax*4]
		cmp [ebx+_SampleFlag],0
		jnz @@good2free
		cmp al,0
		jz @@cannotfree
		jmp @@freeloop
@@good2free:    mov al,0
		call PM_ReleaseModule
		jc @@abort
		ret
@@abort:
@@cannotfree:   stc
		ret
FreeDevRam      endp

;----------------------------------------------------------------------------
Load_Slot       proc near               ;Load filename into slot AL
		inc [ProtectFlag]
@@nochange:     mov al,[CurrentSlot]
		inc al
		cmp al,MAXSLOTS                ;Only MAXSLOTS modules
		ja @@abort
		mov [CurrentSlot],al
		movzx eax,al
		dec eax
		mov ebx,[Song_Table+eax*4]
		mov al,LOAD_DUMP+LOAD_HI        ;Load module
		mov edx,offset SeekFilename

		cmp [ListFlag],0                ;only list if wanted
		jnz @@nolist
		push edx eax ebx
		mov edx,offset Info_Msg17       ;end msg
		mov ax,0901h
		call OutMsg
		mov edx,offset SeekFilename
		mov ax,0700h
		call OutMsg
		pop ebx eax edx
@@nolist:       push eax ebx edx edi
		mov edi,ebx
		add edi,_Title
		call PM_GetFileType             ;first check file type
		pushf
		sub [_himembase],28
		popf
		pop edi edx ebx eax
		jc @@woops
		call InvertPal
		call PM_LoadModule
		call UnInvertPal
		jc @@woops
		dec [ProtectFlag]
		ret
@@woops:        dec [CurrentSlot]
@@abort:        stc
		dec [ProtectFlag]
		ret
Load_Slot       endp
;----------------------------------------------------------------------------
Load_Slots      proc near               ;Load file(s) into multiple slots
		mov [foundcount],0
		mov esi,offset Filename
		cmp byte ptr [esi],0
		jz @notry

		cmp byte ptr [esi],'@'
		jnz @@notfilelist
		call LoadFileList
		clc
		ret
@@notfilelist:  call Scan4File

		mov esi,offset Filename ;first determine if filespec already has .ext
@scannext:      lodsb
		or al,al
		jz @noext
		cmp al,'.'
		jnz @scannext
		jmp @notry

@noext:         call gettodot           ;try .S3M extension
		mov byte ptr [esi-1],'.'
		mov byte ptr [esi],'S'
		mov byte ptr [esi+1],'3'
		mov byte ptr [esi+2],'M'
		call Scan4File

		call gettodot           ;try .MOD extension
		mov byte ptr [esi-1],'.'
		mov byte ptr [esi],'M'
		mov byte ptr [esi+1],'O'
		mov byte ptr [esi+2],'D'
		call Scan4File

		call gettodot           ;try .MTM extension
		mov byte ptr [esi-1],'.'
		mov byte ptr [esi],'M'
		mov byte ptr [esi+1],'T'
		mov byte ptr [esi+2],'M'
		call Scan4File

@notry:         cmp [foundcount],0
		jz @error2
		clc
		ret
@error2:        stc
		ret

gettodot:       mov esi,offset Filename         ;Loop to add ".S3M"
@@addloop:      mov al,[esi]
		inc esi
		cmp al,0
		jz @@finaddot
		cmp al,'.'
		jnz @@addloop
@@finaddot:     ret

Load_Slots      endp

;----------------------------------------------------------------------------
Scan4File       proc near
		mov al,04eh
@nextone:       mov edi,offset Filename
		mov edx,offset ActualFilename
		xor ecx,ecx
		call _findfile
		jc @nomore

		mov esi,offset Filename         ;copy file+path
		mov ebx,esi
@gtnxt:         lodsb
		cmp al,0
		jz @donescan
		cmp al,'\'
		jnz @gtnxt
		mov ebx,esi
		jmp @gtnxt
@donescan:      mov edi,offset SeekFilename
		mov ecx,ebx
		sub ecx,offset Filename
		jz @nocopypath
		mov esi,offset Filename
		rep movsb
@nocopypath:    mov esi,offset ActualFilename
@copyptlp:      lodsb
		stosb
		cmp al,0
		jnz @copyptlp

		call Load_Slot
		jc @not1
		inc [foundcount]

		call PrepareScreen
		cmp [stflag],1          ;handle play on first
		jnz @not1

		call StopCDMode         ;stop cdrom if playing

		mov ax,0101h
		call Play_Slot

		mov [stflag],0

@not1:          mov al,04fh
		jmp @nextone
@nomore:        cmp al,04eh
		jz @error
@@leavescan:    clc
		ret
@error:         stc
		ret
Scan4File       endp

;----------------------------------------------------------------------------
LoadFileList    proc near               ;load modules in file list file
					;  at Filename
		mov ecx,FILENAMESIZE
		cmp [ListFileFree],ecx
		ja @@enuffram
		ret

@@enuffram:     push [ListFileFree]
		sub [ListFileFree],ecx
		mov edi,[ListFileBuffer]
		push edi
		add [ListFileBuffer],ecx
		mov esi,offset Filename         ;preserve current filename buffers
		rep movsb

		mov edx,offset Filename         ;open list file
		cmp byte ptr [edx],'@'
		jnz @@noinc
		inc edx
@@noinc:        call _openfile
		jc @@nolist
		call _filesize                  ;get list size

		cmp [ListFileFree],eax
		jbe @@nolist
		mov ecx,eax                     ;get listbuffer
		sub [ListFileFree],ecx
		mov edx,[ListFileBuffer]
		add [ListFileBuffer],eax
		mov esi,edx                     ;main loading loop
		call _readfile                  ;edx=list buff, ecx=size
		call _closefile

@@listloop:     mov edi,offset Filename         ;find start of name
@@getnext:      lodsb
		dec ecx
		jz @@nolist
		cmp al,32
		jbe @@getnext
		stosb
@@findloop:     lodsb
		dec ecx
		jz @@nolist
		cmp al,32
		jbe @@gotname
		stosb
		jmp @@findloop
@@gotname:      mov byte ptr [edi],0
		push esi ecx                    ;load file
		call Load_Slots
		pop ecx esi
		jmp @@listloop
@@nolist:       pop esi
		mov [ListFileBuffer],esi
		mov edi,offset Filename         ;restore old filename buffers
		mov ecx,FILENAMESIZE
		pop [ListFileFree]

		rep movsb
@@abort:        ret
LoadFileList    endp

;----------------------------------------------------------------------------
Play_Slot       proc near
		inc [ProtectFlag]
		mov [LoopFlag],ah
		push eax
		movzx eax,al
		dec eax
		mov ebx,[Song_Table+eax*4]
		cmp [ebx+_Size],0
		jz @@abort

		cmp [ebx+_SampleFlag],0         ;Make sure enuff DRAM
		jnz @@noprobs

@@checkloop:    push ebx
		call PM_GetDeviceRAM
		pop ebx
		cmp eax,[ebx+_SampleSize]
		jnb @@noprobs2

		push ebx
		movzx eax,[CurrentSlot]         ;Free last module sample ram
@@freeloop:     dec eax
		mov ebx,[Song_Table+eax*4]
		cmp [ebx+_SampleFlag],0
		jnz @@good2free
		cmp al,0
		jz @@cannotfree
		jmp @@freeloop
@@good2free:    mov al,0
		call PM_ReleaseModule
		jc @@cannotfree
		jmp @@freed
@@cannotfree:   pop ebx
		jmp @@abort

@@freed:        pop ebx
		jmp @@checkloop

@@noprobs2:     call InvertPal
		mov al,[Amplification]
		call StopCDMode
		call PM_PlayModule
		mov al,[PlayerVolume]
		call PM_SetMasterVol
		call UnInvertPal
		jmp short @@doneplay
@@noprobs:      mov al,[Amplification]
		call StopCDMode
		call PM_PlayModule
		mov al,[PlayerVolume]
		call PM_SetMasterVol
@@doneplay:
		mov edx,offset _ret
		cmp [LoopFlag],0                ;set loop routine
		jz @disableloop
		mov edx,offset EndModuleHandle
@disableloop:   call PM_SetLoopCode

		pop eax
		mov [CurrentSong],al
		dec [ProtectFlag]
		clc
		ret
@@abort:        pop eax
		dec [ProtectFlag]
		stc
		ret
Play_Slot       endp
;----------------------------------------------------------------------------
EndModuleHandle proc near       ;Handle end of module
		cmp [ProtectFlag],0     ;dont loop only if not critical
		jnz @nostartnext
		pushad
		mov al,[CurrentSong]
		inc al
		cmp al,[CurrentSlot]
		jbe @validslot
		mov al,1
@validslot:     mov ah,1
		call Play_Slot          ;Play next slot
		popad
@nostartnext:   ret
EndModuleHandle endp
;----------------------------------------------------------------------------
InvertPal       proc near       ;Invert palette while dumping
		pushf
		push esi eax ecx edx
		mov dx,03c7h
		mov esi,[Temp_Palette]
		mov al,1                ;First read current pal
		out dx,al
		inc dx
		inc dx
		mov cx,768-3
@@readloop:     in al,dx
		mov [esi],al
		inc esi
		dec cx
		jnz @@readloop

		mov dx,03c8h
		mov esi,[Temp_Palette]
		mov al,1
		out dx,al
		inc dx
		mov cx,255      ;768-3
@@writeloop:    mov al,[esi]
		shr al,1
		inc esi
		out dx,al
		dec cx
		jnz @@writeloop
		pop edx ecx eax esi
		popf
		ret
InvertPal       endp
;----------------------------------------------------------------------------
UnInvertPal     proc near
		pushf
		push esi eax ecx edx
		mov dx,03c8h
		mov esi,[Temp_Palette]
		mov al,1
		out dx,al
		inc dx
		mov cx,255      ;768-3
@@writeloop:    mov al,[esi]
		inc esi
		out dx,al
		dec cx
		jnz @@writeloop
		pop edx ecx eax esi
		popf
		ret
UnInvertPal     endp

;----------------------------------------------------------------------------
ShowHelp        proc near
		mov edx,offset Error_Msg1E      ;space 1st
		xor eax,eax
		call OutMsg
		mov edx,offset Help_Msg1
		mov ax,0900h
		call OutMsg
		mov edx,offset Help_Msg2
@@showloop:     mov ax,0700h
		push edx
		call OutMsg
		pop edx

@@looppt:       mov al,[edx]
		inc edx
		cmp al,0
		jnz @@looppt
		mov al,[edx]
		cmp al,255
		jnz @@showloop
		ret
ShowHelp        endp
;----------------------------------------------------------------------------
ShowDevice      proc near
		mov edx,offset Init_Msg0
		mov ax,0f00h    ;f
		call OutMsg

		mov edx,offset Init_Msg1        ;show device type
		mov ax,0901h
		call OutMsg
		movzx eax,[__SoundDevice]
		mov edx,[DeviceTable+eax*4]
		mov ax,0f00h
		call OutMsg

		mov edx,offset Init_Msg2
		mov ax,0901h
		call OutMsg
		movzx eax,[__PortAddr]
		mov bx,1
		call OutHex
		mov edx,offset Init_Msg3
		mov ax,0901h
		call OutMsg
		movzx eax,[__IntNum]
		mov bx,1
		call OutDec
		mov edx,offset Init_Msg4
		mov ax,0901h
		call OutMsg
		movzx eax,[__DMAChan]

		cmp [__SoundDevice],DEVICE_GUS  ;continue only if not gus
		jnz @@mixedcard
		xor ebx,ebx
		call OutDec
		jmp @@donehere

@@mixedcard:    mov bx,1
		call OutDec
		mov edx,offset Init_Msg6
		mov ax,0901h
		call OutMsg
		movzx eax,[__MixingRate]
		mov bx,1
		call OutDec
		mov edx,offset Init_Msg7        ;show dsp version
		mov ax,0901h
		call OutMsg

		mov edx,offset Letter_V
		mov ax,0f01h
		call OutMsg
		movzx eax,[__Version]
		shr eax,8
		mov bx,1
		call OutDec
		mov edx,offset Letter_Dot
		mov ax,0f01h
		call OutMsg
		movzx eax,[__Version]
		and ax,0ffh
		xor ebx,ebx
		call OutDec

@@donehere:     cmp [CDROM_Flag],0              ;show if cdrom detected
		jz @@nocdrom
		mov edx,offset Init_Msg8
		mov ax,0900h
		call OutMsg

@@nocdrom:      call PrepareScreen
		ret
ShowDevice      endp

;----------------------------------------------------------------------------
PrepareScreen   proc near

		;First wait for retrace

		call VWait

		;call DisableKeyboard
		mov bl,1                ;Set new irq mask
		mov al,1
		call _setirqmask


		cmp [CDROM_Mode],0
		jz @@notcdrom

		mov edi,[ScreenBuffer]         ;Draw header-line
		mov esi,offset HeadLine
		mov ecx,HeadLine_Len
		call DeCrunch
		mov edi,[ScreenBuffer]
		add edi,80*2
		mov esi,offset CDStatusLine
		mov ecx,CDStatusLine_Len
		call DeCrunch
		call @@drawmem

		call CD_Status
		mov [CDROM_StatusFlag],eax

		and eax,CDSTAT_DOOROPEN         ;check if door open
		jz @@cdclosed

		@php ebp,[ScreenBuffer]
		mov edx,offset TxtCD_1
		mov bl,44
		mov bh,3
		mov ah,1+7*16
		call WriteStr

		cmp [CDWasOpenFlag],0
		jnz @@nosong
		inc [CDWasOpenFlag]
		jmp @@nosong

@@cdclosed:     cmp [CDWasOpenFlag],0
		jz @@notnewdisk
		call CD_Get_Audio_Info
		mov [CDWasOpenFlag],0

@@notnewdisk:   mov eax,offset CDPlayStatus     ;draw drive stats
		call CD_GetPos

		;draw cdrom volume
		mov eax,offset CDVolume
		call CD_Get_Volume
		movzx eax,[CDVolume._volume0]

		mov edi,[ScreenBuffer]
		add edi,(1*160)+(73*2)
		mov word ptr [edi],'0'+(7*16+1)*256
		call Bin2Dec
		call @putdec2

		mov al,[CDPlayStatus._track]    ;get current track
		call BCDToDec

		cmp al,[CDROM_STATS._high_audio] ;check track within limits
		ja @@nosong
		cmp al,[CDROM_STATS._low_audio]
		jb @@nosong

		call CD_Set_Track

		call Bin2Dec                    ;draw current track/total tracks
		mov edi,[ScreenBuffer]
		add edi,(2*160)+(44*2)
		call @putdec2
		mov word ptr [edi],'/'+(7*16+1)*256
		inc edi
		inc edi
		movzx eax,[CDROM_STATS._high_audio]
		call Bin2Dec
		call @putdec2

		call CD_Done_Play               ;check if cd is playing
		jz @@nosong

		@php ebp,[ScreenBuffer]
		mov edx,offset TxtCD_2
		mov bl,44
		mov bh,3
		mov ah,1+7*16
		call WriteStr

		mov edi,[ScreenBuffer]
		add edi,(2*160)+(58*2)
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[CDPlayStatus._min]
		cmp eax,10
		jae @@notlittle0
		inc edi
		inc edi
@@notlittle0:   call Bin2Dec
		call @putdec2
		mov word ptr [edi],':'+(7*16+1)*256
		inc edi
		inc edi
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[CDPlayStatus._sec]
		cmp eax,10
		jae @@notlittle1
		inc edi
		inc edi
@@notlittle1:   call Bin2Dec
		call @putdec2

		mov edi,[ScreenBuffer]
		add edi,(3*160)+(58*2)
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[CDPlayStatus._amin]
		cmp eax,10
		jae @@notlittle2
		inc edi
		inc edi
@@notlittle2:   call Bin2Dec
		call @putdec2
		mov word ptr [edi],':'+(7*16+1)*256
		inc edi
		inc edi
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[CDPlayStatus._asec]
		cmp eax,10
		jae @@notlittle3
		inc edi
		inc edi
@@notlittle3:   call Bin2Dec
		call @putdec2

		mov ah,[CDPlayStatus._amin]
		mov al,[CDPlayStatus._asec]
		mov bh,[CDROM_STATS._disk_length_min]
		mov bl,[CDROM_STATS._disk_length_sec]
		call SubtractTime

		mov edi,[ScreenBuffer]
		add edi,(3*160)+(73*2)
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[_Tmp_Min]
		cmp eax,10
		jae @@notlittle4
		inc edi
		inc edi
@@notlittle4:   call Bin2Dec
		call @putdec2
		mov word ptr [edi],':'+(7*16+1)*256
		inc edi
		inc edi
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[_Tmp_Sec]
		cmp eax,10
		jae @@notlittle5
		inc edi
		inc edi
@@notlittle5:   call Bin2Dec
		call @putdec2

		mov edi,offset _Tmp_Min
		mov ax,[CDROM_STATS._current_track]
		call CD_Track_Length
		mov ah,[CDPlayStatus._min]
		mov al,[CDPlayStatus._sec]
		mov bh,[_Tmp_Min]
		mov bl,[_Tmp_Sec]
		call SubtractTime

		mov edi,[ScreenBuffer]
		add edi,(2*160)+(73*2)
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[_Tmp_Min]
		cmp eax,10
		jae @@notlittle6
		inc edi
		inc edi
@@notlittle6:   call Bin2Dec
		call @putdec2
		mov word ptr [edi],':'+(7*16+1)*256
		inc edi
		inc edi
		mov word ptr [edi],'0'+(7*16+1)*256
		movzx eax,[_Tmp_Sec]
		cmp eax,10
		jae @@notlittle7
		inc edi
		inc edi
@@notlittle7:   call Bin2Dec
		call @putdec2

		jmp @@nosong

@@notcdrom:     mov edi,[ScreenBuffer]         ;Draw header-line
		mov esi,offset HeadLine
		mov ecx,HeaderAnsi_Len+HeadLine_Len
		call DeCrunch
		call @@drawmem

		;Show song status
		movzx eax,[CurrentSong]
		or al,al
		jz @@nosong

		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(3*160)+(71*2)          ;Put current song number
		call @putdec2
		call DrawVUBars                 ;Show VU-Bars

		mov ebx,[__CurrentModule]       ;Draw patt num
		movzx eax,[ebx+_MActualPos]        ;_MActualPatt
		inc eax
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(2*160)+(44*2)
		call @putdec2
		mov word ptr [edi],'/'+(7*16+1)*256
		inc edi
		inc edi
		movzx eax,[ebx+_Ordnum]            ;_MActualPos
		call Bin2Dec
		call @putdec2

		movzx eax,byte ptr [ebx+_MActualRow]
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(3*160)+(38*2)
		call @putdec

		movzx eax,byte ptr [ebx+_MCurrentSpd] ;Show speed
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(2*160)+(58*2)
		call @putdec2

		movzx eax,byte ptr [ebx+_MCurrentBPM] ;Show tempo (bpm)
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(3*160)+(58*2)
		call @putdec2

		xor eax,eax                     ;Show master volume
		call PM_GetMasterVol
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(2*160)+(71*2)
		call @putdec2

		;Show module title
		mov edx,[__CurrentModule]
		add edx,_Title
		mov bx,44+256
		mov ah,1+7*16
		@php ebp,[ScreenBuffer]
		call WriteStr

@@nosong:       @rlp edi,0b8000h                ;copy header to screen
		mov esi,[ScreenBuffer]
		mov ecx,160 ;*4/4
		rep movsd

		cmp [PopUpFlag],0
		jnz @@donehere

		cmp [CurrentSong],0
		jz @@nomore                     ;copy extra info if active
		mov ecx,160/4
		mov eax,[__CurrentModule]
		movzx eax,[eax+_TotalChanNum]
		mul ecx
		add ecx,eax
		mov [ExtraOffset],ecx
		rep movsd
		mov [ChangedFlag],1
		jmp @@nocheck
@@nomore:       cmp [ChangedFlag],0
		jz @@nocheck2
		mov [ChangedFlag],0
		@rlp edi,0b8000h+(160*4)        ;blank channel bars on screen
		mov ecx,[ExtraOffset]
		xor eax,eax
		rep stosd
@@nocheck2:     mov [ExtraOffset],0
@@nocheck:
		mov al,[edi+1]          ;blank leftover lines
		shr al,4
		cmp al,7
		jnz @@donehere
		mov ecx,160/4
		xor eax,eax
		rep stosd
		jmp @@nocheck
@@donehere:     ;call EnableKeyboard
		mov bl,1                ;Set new irq mask
		mov al,0
		call _setirqmask

		ret

@putdec:        mov esi,offset asciitbl
		mov cl,8
		mov ah,1+7*16
@@pastem:       mov al,ds:[esi]
		cmp al,' '
		jz @loop
		cmp al,0
		jz @loop
		mov es:[edi],ax
@loop:          inc esi
		inc edi
		inc edi
		dec cl
		jnz @@pastem
		ret
@putdec2:       mov ah,1+7*16
@putdec2B:      mov esi,offset asciitbl
		mov cl,8
@@pastem2:      mov al,ds:[esi]
		cmp al,' '
		jz @loop2
		cmp al,0
		jz @loop2
		mov es:[edi],ax
		inc edi
		inc edi
@loop2:         inc esi
		dec cl
		jnz @@pastem2
		ret

@@drawmem:      call _lomemsize
		call Bin2Dec                    ; put amount of low memory
		mov edi,[ScreenBuffer]
		add edi,(1*160)+(23*2)
		call @putdec

		call _himemsize                 ; put amount of extended memory
		call Bin2Dec
		mov edi,[ScreenBuffer]
		add edi,(2*160)+(23*2)
		call @putdec

		mov edi,[ScreenBuffer]
		add edi,(3*160)+(23*2)          ;Put amount of device ram
		call PM_GetDeviceRAM
		call Bin2Dec
		call @putdec
		ret
PrepareScreen   endp

;----------------------------------------------------------------------------
DrawVUBars      proc near               ;Draw VU-Bar info for module EBX
		mov edi,[ScreenBuffer]         ;Draw info-line
		add edi,160*4
		mov esi,offset InfoAnsi
		mov ecx,InfoAnsi_Len
		call DeCrunch

		mov ebx,[__CurrentModule]
		mov edi,[ScreenBuffer]
		add edi,160*5
		mov cl,[ebx+_TotalChanNum]
		mov esi,[__ChannelData]
@@drawvubar:    push ecx                        ;loop to draw all vu-bar stats
		push edi

		mov eax,071007100h              ;set backcolour
@@setcol:       cmp [esi+_ActiveFlag],0
		jnz @@chanactive
		mov eax,070007000h
@@chanactive:   mov ecx,40
		rep stosd
		pop edi

		push edi
		add edi,2                       ;draw sample name
		movzx eax,[esi+_SampleNum]
		or al,al
		jz @@nosample
		cmp al,255
		jz @@nosample
		push esi
		mov ebx,[__CurrentModule]
		mov esi,[ebx+_Pointer]
		movzx ecx,[ebx+_Insnum]
		movzx ebx,[ebx+_Ordnum]
		add ebx,60h
		add ebx,esi
		dec eax
		add eax,eax
		add ebx,eax
		movzx eax,word ptr [ebx]
		shl eax,4
		add esi,eax
		add esi,30h
		mov cl,27
@@nextone:      movsb
		inc edi
		dec cl
		jnz @@nextone
		pop esi
@@nosample:     pop edi

		push edi
		add edi,29*2
		movzx eax,[esi+_TargetNote]    ;draw note number
		or eax,eax
		jz @@nonote                     ;note
		and al,0fh
		add al,al
		push esi
		mov esi,offset NoteText
		add esi,eax
		movsb
		inc edi
		movsb
		inc edi
		pop esi
		mov al,[esi+_TargetNote]       ;octave
		shr al,4
		add al,'0'
		stosb
@@nonote:       pop edi

		push edi
		add edi,34*2
		movzx eax,[esi+_CurrentVol]     ;draw volume
		mov dl,10
		div dl
		add al,'0'
		add ah,'0'
		mov [edi],al
		mov [edi+2],ah
		pop edi

		push edi
		add edi,38*2
		push edi
		mov al,0feh             ;set vu bar colours
		mov ecx,16
@@nextbar:      stosb
		inc edi
		loop @@nextbar
		pop edx
		inc edx

		mov edi,offset VU_Colour_Table
		movzx eax,[esi+_VUBarLevel]
		or al,al                ;check if zero vol
		jz @@none
		shr al,2
		inc al
		cmp al,16
		jb @@ok                 ;limit to 16 blocks of bar
		mov al,16
@@ok:           mov ecx,eax
		xor eax,eax
		xor ebx,ebx
@@setcolp:      mov bl,[edi+eax]
		mov [edx+eax*2],bl
		inc eax
		loop @@setcolp
@@none:         pop edi

		push edi
		add edi,55*2

		movzx eax,[esi+_PanPosition]    ;draw pan position
		mov dl,3
		mul dl
		add eax,offset Pan_Table
		push esi
		mov esi,eax
@@nextchar:     movsb
		inc edi
		dec dl
		jnz @@nextchar
		pop esi
		pop edi

		push edi
		add edi,59*2
		movzx eax,byte ptr [esi+_CMDVal]
		cmp al,19               ;handle special commands
		jnz @@nots
		mov al,[esi+_CMDData]
		shr al,4
		mov edx,[SFX_Info_Table+eax*4]
		jmp short @@writetxt
@@nots:         cmp al,26               ;handle normal commands
		jae @@nofxt
		mov edx,[FX_Info_Table+eax*4]
@@writetxt:     or edx,edx
		jz @@nofxt
		push esi
		mov esi,edx
@@nextbytet:    lodsb
		cmp al,0
		je @@donefxt
		stosb
		inc edi
		jmp short @@nextbytet
@@donefxt:      pop esi
@@nofxt:        pop edi
		pop ecx
		add edi,160
		add esi,ChanDataSize
		dec cl
		jnz @@drawvubar
		ret
DrawVUBars      endp

;----------------------------------------------------------------------------
EndScreen       proc near
		mov [v86r_ax],0003h     ;Use bios to set videomode
		mov al,10h
		int 33h
		call DrawStarAnsi

		@rlp edi,0b8000h+(9*160) ;Draw contacts
		mov ecx,ContactAnsi_Len
		mov esi,offset ContactAnsi
		call DeCrunch

		mov [v86r_ax],0200h     ;Set cursor pos
		mov [v86r_dx],1400h
		mov [v86r_bx],0
		mov al,10h
		int 33h

		ret
EndScreen       endp

;----------------------------------------------------------------------------
DrawStarAnsi    proc near
		@rlp edi,0b8000h
		mov ecx,EndAnsi_Len     ;Draw log off ansi
		mov esi,offset EndAnsi
		call DeCrunch
		ret
DrawStarAnsi    endp

;----------------------------------------------------------------------------
DeCrunch        proc near               ;unpack thedraw crunched text
		;pushad                  ;Save registers.
		push eax ebx ecx edx edi
		jcxz @Done

		mov edx,edi             ;Save X coordinate for later.
		xor eax,eax             ;Set Current attributes.

@loopa:         lodsb                   ;Get next character.
		cmp al,32               ;If a control character, jump.
		jc @ForeGround
		stosw                   ;Save letter on screen.
@Next:          loop @loopa
		jmp short @Done

@ForeGround:    cmp al,16               ;If less than 16, then change the
		jnc @BackGround         ;foreground color.  Otherwise jump.
		and ah,0f0h             ;Strip off old foreground.
		or ah,al
		jmp @Next

@BackGround:    cmp al,24               ;If less than 24, then change the
		jz @NextLine            ;background color.  If exactly 24,
		jnc @FlashBitToggle     ;then jump down to next line.
		sub al,16               ;Otherwise jump to multiple output
		shl al,4                ;routines.
		and ah,8fh              ;Strip off old background.
		or ah,al
		jmp @Next

@NextLine:      add edx,160             ;If equal to 24,
		mov edi,edx             ;then jump down to
		jmp @Next

@FlashBitToggle: cmp al,27              ;Does user want to toggle the blink
		jc @MultiOutput         ;attribute?
		jnz @Next
		xor ah,128              ;Done.
		jmp @Next

@MultiOutput:   cmp al,25               ;Set Z flag if multi-space output.
		mov ebx,ecx             ;Save main counter.
		lodsb                   ;Get count of number of times
		mov cl,al               ;to display character.
		mov al,32
		jz @StartOutput         ;Jump here if displaying spaces.
		lodsb                   ;Otherwise get character to use.
		dec ebx                 ;Adjust main counter.

@StartOutput:   xor ch,ch
		inc ecx
		rep stosw
		mov ecx,ebx
		dec ecx                 ;Adjust main counter.
		loopnz @loopa           ;Loop if anything else to do...

@Done:          ;popad                   ;Restore registers.
		pop edi edx ecx ebx eax
		ret
DeCrunch        endp

;----------------------------------------------------------------------------
; Put ASCIIZ string to screen
; In:
;   AH - attribute
;   BL - X
;   BH - Y
;   EDX -> ASCIIZ string
;   EBP -> realmode ram offset of screen (0b8000h)
; Returns: CX=Length of string written
;----------------------------------------------------------------------------
WriteStr        proc near
		xor ecx,ecx
		push eax esi edi
		movzx edi,bh
		imul edi,160
		movzx esi,bl
		shl esi,1
		add edi,ebp
		lea edi,[edi+esi]
		sub edi,_code32a
		mov esi,edx
putstrl0:       lodsb
		or al,al
		jz short putstrd
		inc ecx
		stosw
		jmp putstrl0
putstrd:        pop edi esi eax
		ret
WriteStr        endp

;----------------------------------------------------------------------------
OutMsg          proc near               ;Write text to screen
		push eax
		push eax
		or ah,ah
		jnz colok
		mov ah,0fh
colok:          mov bl,[Scr_Xpos]
		mov bh,[Scr_Ypos]
		push ebp
		mov ebp,0b8000h
		call WriteStr
		pop ebp
		pop eax
		or al,al                ;Check for linefeed (AH=0)
		jz @@normal
		add [Scr_Xpos],cl
		pop eax
		ret
@@normal:       mov [Scr_Xpos],0
		inc [Scr_Ypos]
		mov al,[Last_Row]
		cmp [Scr_Ypos],al
		jae @@scrollscr
		pop eax
		ret
@@scrollscr:    call ScrollScreen
		pop eax
		ret
OutMsg          endp
;----------------------------------------------------------------------------
OutDec          proc near               ;Write a DECIMAL 32-bit number
		call Bin2Dec
		mov al,160
		mul [Scr_Ypos]
		push ebx
		mov bh,0
		mov bl,[Scr_Xpos]
		shl bx,1
		add ax,bx
		pop ebx
		movsx eax,ax
		@rlp edi,0b8000h
		add edi,eax

		mov cl,8
		mov esi,offset asciitbl
@@findnum:      mov al,[esi]
		inc esi
		mov bh,cl
		cmp al,' '
		jnz @@foundit
		dec cl
		jnz @@findnum
		ret
@@foundit:      mov [edi],al
		inc edi
		mov byte ptr [edi],15;15
		inc edi
		mov al,[esi]
		inc esi
		dec cl
		jnz @@foundit

		or bl,bl
		jz @@normalinc
		add [Scr_Xpos],bh
		ret
@@normalinc:    mov [Scr_Xpos],0
		inc [Scr_Ypos]
		mov al,[Last_Row]
		cmp [Scr_Ypos],al
		jz @@scrollscr
		ret
@@scrollscr:    call ScrollScreen
		ret
OutDec          endp
;----------------------------------------------------------------------------
OutHex          proc near
		call Bin2Hex
		mov al,160
		mul [Scr_Ypos]
		push ebx
		mov bh,0
		mov bl,[Scr_Xpos]
		shl bx,1
		add ax,bx
		pop ebx
		movsx eax,ax
		@rlp edi,0b8000h
		add edi,eax

		mov cl,8
		mov esi,offset asciitbl
@@findnum:      mov al,[esi]
		inc esi
		mov bh,cl
		cmp al,'0'
		jnz @@foundit
		dec cl
		jnz @@findnum
		ret
@@foundit:      mov [edi],al
		inc edi
		mov byte ptr [edi],15
		inc edi
		mov al,[esi]
		inc esi
		dec cl
		jnz @@foundit

		or bl,bl
		jz @@normalinc
		add [Scr_Xpos],bh
		ret
@@normalinc:    mov [Scr_Xpos],0
		inc [Scr_Ypos]
		mov al,[Last_Row]
		cmp [Scr_Ypos],al
		jz @@scrollscr
		ret
@@scrollscr:    call ScrollScreen
		ret
OutHex          endp
;----------------------------------------------------------------------------
Bin2Dec         proc near               ;Convert EAX to decimal
		;pushad
		push eax ebx ecx edx edi
		mov edi,offset asciitbl ;first blank buffer
		push eax
		xor eax,eax
		mov [edi],eax
		mov [edi+4],eax
		mov [edi+8],ax
		pop eax
		xor edx,edx
		mov ebx,edi
		add edi,8
		mov byte ptr ds:[edi],0
		dec edi
ssss:           mov ecx,10
		div ecx
		add dl,30h
sssss:          mov ds:[edi],dl
		dec edi
		xor edx,edx
		or eax,eax
		jne ssss
		mov dl,' '
		cmp ebx,edi
		jbe sssss
		;popad
		pop edi edx ecx ebx eax
		ret
Bin2Dec         endp
;----------------------------------------------------------------------------
Bin2Hex         proc near               ;Convert EAX to hex
		;pushad
		push eax ebx ecx edi
		mov edi,offset asciitbl

		mov cl,8                ;Convert to hex
@@hexloop:      rol eax,4
		mov bl,al
		and ebx,0fh
		mov bl,[Hex_values+ebx]
		mov [edi],bl
		inc edi
		dec cl
		jnz @@hexloop

		mov byte ptr ds:[edi],0
		;popad
		pop edi ecx ebx eax
		ret
Bin2Hex         endp
;----------------------------------------------------------------------------
ScrollScreen    proc near
		push esi edi
@@normal:       mov al,[Last_Row]
		dec al
		mov [Scr_Ypos],al       ;Scroll screen up
		@rlp edi,0b8000h+(160*4)
		@rlp esi,0b8000h+(160*5)
		mov eax,[ExtraOffset]   ;account for variable size

		push eax ebx edx            ;mov ecx,(160*(50-5))/4
		movzx eax,[Last_Row]
		sub eax,5
		mov ebx,160/4
		mul ebx
		mov ecx,eax
		pop edx ebx eax

		sub ecx,eax
		shl eax,2
		add edi,eax
		add esi,eax
		cmp ecx,160*50/4        ;protect from crash
		ja @@end
		rep movsd
		xor eax,eax
		mov ecx,160/4
		rep stosd
@@end:          pop edi esi
		ret
ScrollScreen    endp
;----------------------------------------------------------------------------
VWait           proc near               ;Wait for verticle retrace

		mov dx,3dah
VRT:            in al,dx
		test al,8
		jnz VRT                 ;wait until Verticle Retrace starts
NoVRT:          in al,dx
		test al,8
		jz NoVRT                ;wait until Verticle Retrace Ends
		ret
VWait           endp

;----------------------------------------------------------------------------
StopCDMode      proc near               ;cancel CD mode if active & stop playing
		cmp [CDROM_Mode],0
		jnz @@cdmode
		ret
@@cdmode:       pushad
		call CD_Stop_Audio
		;mov al,DOORUNLOCK
		;call CD_Lock
		mov [CDROM_Playing],0
		mov [CDROM_Mode],0
		popad
		ret
StopCDMode      endp

BCDToDec        proc near       ;convert BCD value in AL to real binary value
		push ebx eax
		movzx eax,al
		mov bl,10
		shr al,4
		mul bl
		pop ebx
		and bl,0fh
		add al,bl
		pop ebx
		ret
BCDToDec        endp

SubtractTime    proc near       ;subtract times AH:AL from BH:BL
		mov [_Tmp_Sec],al
		mov [_Tmp_Min],ah

		;first convert max time to secs
		mov eax,60
		mul bh
		movzx ebx,bl
		add eax,ebx
		push eax

		;then convert min time to secs
		mov eax,60
		mul [_Tmp_Min]
		movzx ebx,[_Tmp_Sec]
		add eax,ebx

		pop ebx                 ;max-min, ebx=remaining secs
		sub ebx,eax

		mov eax,ebx
		mov ebx,60
		xor edx,edx
		div ebx

		mov [_Tmp_Min],al
		mov [_Tmp_Sec],dl

		ret
SubtractTime    endp

;----------------------------------------------------------------------------
code32          ends
		end
