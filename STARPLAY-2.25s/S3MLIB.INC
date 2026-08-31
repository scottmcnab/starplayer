; S3MLIB.INC
; Includes for starplayer music library code
; Copyright (c) Scott McNab (jedi/oxygen), 1994-1996
; jedi@tartarus.uwa.edu.au

		extrn PM_InitSystem:NEAR
		extrn PM_LoadModule:NEAR
		extrn PM_PlayModule:NEAR
		extrn PM_StopModule:NEAR
		extrn PM_ReleaseModule:NEAR
		extrn PM_CloseSystem:NEAR
		extrn PM_GetDeviceRAM:NEAR
		extrn PM_GetMasterVol:NEAR
		extrn PM_SetMasterVol:NEAR
		extrn PM_SetLoopCode:NEAR
		extrn PM_GetFileType:NEAR

ChannelData     struc
_ChannelNumber  db      ?               ;Data for which channel (0-15)
_SampleNum      db      ?               ;Current sample for channel (1-100)
_ChannelFlag    db      ?               ;Flag for channel updates
_SampleOffset   dw      ?               ;Sample offset to start new sample
_CurrentVol     db      ?               ;Current channel volume (0-64)
_ActualVol      db      ?               ;Actual channel volume used (0-64)
_CurrentNote    db      ?               ;Current note byte (oct,note)
_CurrentPeriod  dd      ?               ;Current ST3 note period
_TargetNote     db      ?               ;Target note byte (oct,note) - info only
_TargetPeriod   dd      ?               ;Target ST3 note period
_ActualPeriod   dd      ?               ;Actual ST3 note period used
_CommandValue   db      ?               ;Command value for current note
_DataValue      db      ?               ;Data value for current note
_PortaValue     db      ?               ;Current portamento value
_VolSlideValue  db      ?               ;Current volume slide value
_VibValue       db      ?               ;Current vibrato value
_VibCount       db      ?               ;Counter for vibrato command
_VibTable       db      ?               ;Current Vibrato table used for channel
_TremTable      db      ?               ;Current Tremolo table used
_RetrigValue    db      ?               ;Current retrigger value
_C4SPD          dd      ?               ;Middle-c freq for last sample
_SampleVolume   db      ?               ;Volume of last used sample
_PanPosition    db      ?               ;Pan position for channel
_SpecialValue   db      ?               ;Value for Sxx commands
_TremorCount    db      ?               ;Counter for tremor
_TremorFlag     db      ?               ;Flag for tremor
_ArpCount       db      ?               ;Counter for arpeggio
_ArpValue       db      ?               ;Value for old arpeggio
_OffsetValue    db      ?               ;Value for old sample offset
_GlissFlag      db      ?               ;Glissando on/off flag
_VUBarLevel     db      ?               ;Vol Level for VU bars
_CMDVal         db      ?               ;Command value for host program
_CMDData        db      ?               ;Data value for host program
_ActiveFlag     db      ?               ;Flag for channel active (info only)
ChanDataSize    equ     $-_ChannelNumber;Defines size of mem for channel data
ChannelData     ends

;Channel Update flag values

_CHN_NewVol     equ     00000001b       ;Change/New chan volume
_CHN_NewSamp    equ     00000010b       ;New sample/sample point
_CHN_NewPitch   equ     00000100b       ;Change/New sample pitch
_CHN_NewPan     equ     00001000b       ;Set pan position for channel
_CHN_NewBPM     equ     00010000b       ;Change song BPM setting

_ICHN_NewVol    equ     11111110b
_ICHN_NewSamp   equ     11111101b
_ICHN_NewPitch  equ     11111011b
_ICHN_NewPan    equ     11110111b
_ICHN_NewBPM    equ     11101111b

;---------------------------------------
Module          struc
_Pointer        dd      ?               ;Pointer to module in CODE32 segment
_Size           dd      ?               ;Module memory size (same as file for s3m)
;_SizeFile       dd      ?               ;Module disk file size
_Type           dw      ?               ;Type of module
_DeviceType     db      ?               ;Type of sound-device for module
_Title          db      29 dup(?)       ;Song title
_SampleFlag     db      ?               ;Sample data in GUS ram?
_SampleSize     dd      ?               ;Size of sample data

_Ordnum         dw      ?               ;Number of orders in file (should be even!)
_Insnum         dw      ?               ;Number of instruments in file
_Patnum         dw      ?               ;Number of patterns in file

;_SampNum        dw      ?               ;Number of samples in file (=inst for S3M)

_globalvol      db      ?               ;Global module volume
_mastervol      db      ?               ;Master module volume (bit 7: 1=stereo,0=mono)
_initialspd     db      ?               ;Module initial speed
_initialBPM     db      ?               ;Module initial tempo
_stereoflag     db      ?               ;Flag for stereo module
_generalflags   dw      ?               ;General flags (eg. amiga limits)
_RealGlobalVol  db      ?               ;Current module volume

_TotalChanNum   db      ?               ;Total number of active channels

_MCurrentSpd    db      ?               ;Current module speed
_MCurrentBPM    db      ?               ;Current module tempo

_BreakToRow     db      ?               ;Break to this row
_MCurrentRow    db      ?               ;Current row being played
_MRowPointer    dd      ?               ;Pointer to current row in s3m track
_MCurrentPos    dw      ?               ;Current pos in pattern list
_MCurrentPatt   db      ?               ;Current pattern being played
_MCurrentTick   db      ?               ;Current pos in pattern list
_MRowDelay      db      ?               ;Counter for row delay

_MRowLoopStart  db      ?               ;Row position for row loops
_MRowLoopCount  db      ?               ;Counter for row loops

_MActualRow     db      ?               ;Actual row (for track viewing)
_MActualPos     dw      ?               ;Actual pos
_MActualPatt    db      ?               ;Actual patt
_MActualTick    db      ?               ;Actual tick

;_XMPattTable    dd      ?               ;Pointer to XM pattern table
;_XMInstTable    dd      ?               ;Pointer to XM instrument table
;_XMSampTable    dd      ?               ;Pointer to XM sample table
M_Struc_Size    equ     $-_Pointer
Module          ends

		extrn __SystemStatus:BYTE
		extrn __SoundDevice:BYTE
		extrn __PortAddr:WORD
		extrn __IntNum:BYTE
		extrn __DMAChan:BYTE
		extrn __DeviceRAM:DWORD
		extrn __CurrentModule:DWORD
		extrn __DMAFlag:BYTE
		extrn __MixingRate:WORD
		extrn __Version:WORD
		extrn __ChannelData:DWORD

DEVICE_UNDEF    equ     0               ;Undefined sound-device
DEVICE_GUS      equ     1               ;Sound device is GUS
DEVICE_SBMONO   equ     2               ;Sound device is SB-MONO
DEVICE_AUTO     equ     255             ;Sound device is Autodetected

SYS_UNINIT      equ     0               ;System is not initialised
SYS_INIT        equ     1               ;System is initialised
SYS_PLAY        equ     2               ;System is currently playing

LOAD_HI         equ     1               ;Load module into hi-mem
LOAD_LO         equ     2               ;Load module into lo-mem
LOAD_DUMP       equ     4               ;Dump samples after loading
LOAD_FREE       equ     8               ;Free sample ram after dumping

TYPE_NONE       equ     0               ;Module is not loaded
TYPE_S3M        equ     1               ;Module type is S3M
TYPE_MOD        equ     2               ;Module type is MOD (converts to S3M)
TYPE_MTM        equ     3               ;Module type is MTM (converts to S3M)

