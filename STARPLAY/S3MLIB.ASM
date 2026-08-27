; S3MLIB - PMODE/W version
;----------------------------------------------------------------------------
		.386p
		jumps
		locals

;----------------------------------------------------------------------------
		public __SystemStatus   ;Public variables
		public __SoundDevice
		public __PortAddr
		public __IntNum
		public __DMAChan
		public __DeviceRAM
		public __CurrentModule
		public __DMAFlag
		public __MixingRate
		public __Version
		public __ChannelData

		public PM_InitSystem    ;Public sub-routines
		public PM_LoadModule
		public PM_PlayModule
		public PM_StopModule
		public PM_ReleaseModule
		public PM_CloseSystem
		public PM_GetDeviceRAM
		public PM_GetMasterVol
		public PM_SetMasterVol
		public PM_SetLoopCode
		public PM_GetFileType

		extrn _lomemsize:NEAR
		extrn _himemsize:NEAR
		extrn _gethimem:NEAR
		extrn _freehimem:NEAR

; --- Macros for convinent coding ---

@outb           macro val
		mov al,&val
		out dx,al
endm

@outbyte        macro port, val
		mov dx,&port
		mov al,&val
		out dx,al
endm

@outword        macro port, val
		mov dx,&port
		mov ax,&val
		out dx,ax
endm

;Convert to GUS macros
@ADDRHIGH       macro
		shr eax,7
		and eax,00001fffh
endm

@ADDRLOW        macro
		and eax,0000007fh
		shl eax,9
endm

;----------------------------------------------------------------------------
; DATA
;----------------------------------------------------------------------------
_DATA           segment use32 dword public 'DATA'
;----------------------------------------------------------------------------
; Music Library Data Goes Here
;----------------------------------------------------------------------------

		extrn PSP_Selector:WORD

;Equates:
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

;GUS equates
MASTER_RESET    equ     04ch
MIDI_RESET      equ     03h
DMA_CONTROL     equ     041h
SET_DMA_ADDRESS equ     042h
TIMER_CONTROL   equ     045h
TIMER1          equ     046h
TIMER2          equ     047h

SAMPLE_CONTROL  equ     049h
SET_VOICES      equ     0eh
SAMPLE_CONTROL  equ     049h
GET_IRQV        equ     08fh
SET_CONTROL     equ     0h
SET_VOLUME_CONTROL equ  0dh
SET_FREQUENCY   equ     01h
SET_START_HIGH  equ     02h
SET_START_LOW   equ     03h
SET_END_HIGH    equ     04h
SET_END_LOW     equ     05h
SET_VOLUME_RATE equ     06h
SET_VOLUME_START equ    07h
SET_VOLUME_END  equ     08h
SET_VOLUME      equ     09h
SET_ACC_HIGH    equ     0ah
SET_ACC_LOW     equ     0bh
SET_BALANCE     equ     0ch

GET_CONTROL     equ     80h
GET_FREQUENCY   equ     81h
GET_START_HIGH  equ     82h
GET_START_LOW   equ     83h
GET_END_HIGH    equ     84h
GET_END_LOW     equ     85h
GET_VOLUME_RATE equ     86h
GET_VOLUME_START equ    87h
GET_VOLUME_END  equ     88h
GET_VOLUME      equ     89h
GET_ACC_HIGH    equ     8ah
GET_ACC_LOW     equ     8bh
GET_BALANCE     equ     8ch
GET_VOL_CONTROL equ     8dh

GF1_MIDI_CTRL   equ     0100h
GF1_VOICESELECT equ     0102h
GF1_REG_SELECT  equ     0103h
GF1_DATA_LOW    equ     0104h
GF1_DATA_HI     equ     0105h
GF1_IRQ_STAT    equ     0006h
GF1_TIMER_CTRL  equ     0008h
GF1_TIMER_DATA  equ     0009h
GF1_IRQ_CTRL    equ     000bh

GF1_DRAM        equ     0107h

VOICE_STOPPED   equ     001h            ;voice has stopped
STOP_VOICE      equ     002h            ;stop voice
VC_DATA_TYPE    equ     004h            ;0=8 bit,1=16 bit
VC_LOOP_ENABLE  equ     008h            ;1=enable
VC_BI_LOOP      equ     010h            ;1=bi directional looping
VC_WAVE_IRQ     equ     020h            ;1=enable voice's wave irq
VC_DIRECT       equ     040h            ;0=increasing,1=decreasing
VC_IRQ_PENDING  equ     080h            ;1=wavetable irq pending

VOLUME_STOPPED  equ     001h            ;volume has stopped
STOP_VOLUME     equ     002h            ;stop volume
VC_ROLLOVER     equ     004h            ;Roll PAST end & gen IRQ
VL_LOOP_ENABLE  equ     008h            ;1=enable
VL_BI_LOOP      equ     010h            ;1=bi directional looping
VL_WAVE_IRQ     equ     020h            ;1=enable voice's wave irq
VL_DIRECT       equ     040h            ;0=increasing,1=decreasing
VL_IRQ_PENDING  equ     080h            ;1=wavetable irq pending

GF1_MASTER_RESET equ    001h            ;0=hold in reset
GF1_OUTPUT_ENABLE equ   002h            ;enable output
GF1_MASTER_IRQ  equ     004h            ;master IRQ enable

MIX_SEL_PORT    equ     0506h           ;Offset from base port
MIX_DATA_PORT   equ     0106h           ;Offset from base port
MIX_CTRL_LEFT   equ     00h
MIX_CTRL_RIGHT  equ     01h
MIX_ATTN_LEFT   equ     02h
MIX_ATTN_RIGHT  equ     03h
FLIP_REV        equ     5

;Errors:
_NO_ERROR       equ     0               ;No Error
_OPEN_ERROR     equ     1               ;Unable to open/(close) file
_READ_ERROR     equ     2               ;Error reading, fseek etc.
_MEM_ERROR      equ     3               ;Insufficient mem
_TYPE_ERROR     equ     4               ;Unrecognisable file type
_CORRUPT_ERROR  equ     5               ;File corrupt in some way


TYPE_NONE       equ     0               ;Module is not loaded
TYPE_S3M        equ     1               ;Module type is S3M
TYPE_MOD        equ     2               ;Module type is MOD (converts to S3M)
TYPE_MTM        equ     3               ;Module type is MTM (converts to S3M)

;---------------------------------------
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
_VUBarLevel     db      ?               ;Vol Level for VU bars (info only)
_CMDVal         db      ?               ;Command value for host program (info only)
_CMDData        db      ?               ;Data value for host program (info only)
_ActiveFlag     db      ?               ;Flag for channel active (info only)
ChanDataSize    equ     $-_ChannelNumber;Defines size of mem for channel data
ChannelData     ends

;Channel Update flag values

_CHN_NewVol     equ     00000001b       ;Change/New chan volume
_CHN_NewSamp    equ     00000010b       ;New sample/sample point
_CHN_NewPitch   equ     00000100b       ;Change/New sample pitch
_CHN_NewPan     equ     00001000b       ;Set pan position for channel
_CHN_NewBPM     equ     00010000b       ;Change song BPM setting
_CHN_StopVoice  equ     10000000b       ;*Special: only for gus

_ICHN_NewVol    equ     11111110b
_ICHN_NewSamp   equ     11111101b
_ICHN_NewPitch  equ     11111011b
_ICHN_NewPan    equ     11110111b
_ICHN_NewBPM    equ     11101111b
_ICHN_StopVoice equ     01111111b       ;*Special: only for gus

;---------------------------------------
Module          struc
_Pointer        dd      ?               ;Pointer to module in CODE32 segment
_Size           dd      ?               ;Module file size
_Type           dw      ?               ;Type of module
_DeviceType     db      ?               ;Type of sound-device for module
_Title          db      29 dup(?)       ;Song title
_SampleFlag     db      ?               ;Sample data in GUS ram?
_SampleSize     dd      ?               ;Size of sample data

_Ordnum         dw      ?               ;Number of orders in file (should be even!)
_Insnum         dw      ?               ;Number of instruments in file
_Patnum         dw      ?               ;Number of patterns in file

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
_MCurrentPatt   db      ?               ;Current row being played
_MCurrentTick   db      ?               ;Current pos in pattern list
_MRowDelay      db      ?               ;Counter for row delay

_MRowLoopStart  db      ?               ;Row position for row loops
_MRowLoopCount  db      ?               ;Counter for row loops

_MActualRow     db      ?               ;Actual row (for track viewing)
_MActualPos     dw      ?               ;Actual pos
_MActualPatt    db      ?               ;Actual patt
_MActualTick    db      ?               ;Actual tick
M_Struc_Size    equ     $-_Pointer
Module          ends

;Copyright Info (hehehe)
ID_Text         db      '-+ oxyplay +- music library (c) scott mcnab 1994/95 (jedi / oxygen)',0

LoaderCodes     db      4 dup(?)

;Gravis Ultrasound device variables
_GUS_NumVoices  dw      14              ;MaxActiveVoice (totalvoices-1)
_GUS_DivisorValue dw    1               ;GUS frequency divisor
_select         dw      ?
_data_hi        dw      ?
_data_low       dw      ?
_mix_addr       dw      ?
_mix_data       dw      ?
_mix_revision   db      ?               ;ICS mixer revision number
_DRAM_Counter   dd      ?               ;Counter for DRAM loading
_GUS_DramBegin  dd      ?               ;Values for sample playing
_GUS_DramStart  dd      ?
_GUS_DramEnd    dd      ?
_GUS_Mode       db      ?               ;Mode value for sample
_GUS_Start      dw      ?
_GUS_End        dw      ?
_GUS_TimerTotal dw      ?               ;Total count for GUS timer
_GUS_TimerLeft  dw      ?               ;GUS timer ticks left b4 service int
_GUS_MaskDWord  dd      ?               ;Channel flags for IRQ handler (vol ramp)
_GUS_DMABuffer  dd      ?               ;Pointer to DMA lo-mem buffer
_GUS_DMABufLen  dd      ?               ;Length of DMA lo-mem buffer
_GUS_DMACount   db      ?               ;Counter to indicate DMA irqs
_GUS_LowMemTot  dd      ?               ;Number of bytes allocated in low mem

;----------------------
MixData         struc           ;structure for a channel to be mixed
_Mix_CurrentPtr dd      ?               ;Pointer to current sample
_Mix_LoopEnd    dd      ?               ;End pointer
_Mix_LoopLen    dd      ?               ;Loop length
_Mix_LowSpeed   dd      ?               ;Low speed rate
_Mix_HighSpeed  dd      ?               ;High speed rate
_Mix_Count      dd      ?               ;Mixing counter
_Mix_ScaleRate  dd      ?               ;Rate for scaling
_Mix_Volume     dd      ?               ;Volume of output (only hi-byte)
_Mix_PanPos     db      ?               ;Pan position (128=centre)
_Mix_ActiveFlag db      ?               ;Is voice active flag?
Mix_Size        equ     $-_Pointer
MixData         ends

;Soundblaster device variables
_SB_BufferSize  dw      ?               ;Size of DMA buffers
_SB_Buffer_0    dd      ?               ;Ptr to 1st buffer
_SB_Buffer_1    dd      ?               ;Ptr to 2nd buffer
_SB_CurrentBuf  db      ?               ;Current buffer under DMA
_SB_TimeConst   db      ?               ;Time constant for SB DSP

_SB_LowMemTotal dd      ?               ;Total bytes allocated in low memory
_SB_HiMemTotal  dd      ?               ;Hi-mem mixing buffers

_SB_GapLength   dd      ?               ;Number of bytes to mix between tracking
_SB_GapCount    dd      ?               ;Number of bytes remaining to be mixed
_SB_BufCount    dd      ?               ;Number of bytes left to fill in buf

_SB_NumChans    db      ?               ;Number of channels to mix
TempMixCount    dd      ?               ;temp mixing value

VolumeTable     dd      ?               ;Pointer to volume table
PostTable       dd      ?               ;Pointer to post-processing table
MixingTable     dd      ?               ;Pointer to mixing table

SB_Tables       db      Mix_Size*32 dup(?)      ;Tables for channel data
;----------------------

Ultrasnd        db      'ULTRASND='
ULTRALEN        EQU     $-Ultrasnd

Blaster         db      'BLASTER='
BLASTERLEN      EQU     $-Blaster

;DMAC variables
DMAChannel      db      1               ;DMA channel to use
DMABaseAdd      dw      ?               ;DMA base register address
DMAPageReg      dw      ?               ;DMA page register address

DMA_Page        db      ?               ;temporary variables
DMA_Offset      dw      ?

		;      page, address
DMAAddrTable    dw      87h, 0          ;chan 0
		dw      83h, 2          ;chan 1
		dw      81h, 4          ;chan 2
		dw      82h, 6          ;chan 3
		dw      8Fh, 0C0h       ;chan 4
		dw      8Bh, 0C4h       ;chan 5
		dw      89h, 0C8h       ;chan 6
		dw      8Ah, 0CCh       ;chan 7

;GUS init variables

dma_reg_list    db  0,1,0,2,0,3,4,5
irq_reg_list    db  0,0,1,3,0,2,0,4,0,0,0,5,6,0,0,7

_GF1            db      ?
_midi           db      ?
_irq_control    db      ?
_dma_control    db      ?


;Global system variables

__SystemStatus  db      ?               ;Initialised flag
__SoundDevice   db      DEVICE_UNDEF    ;Sound device type
__PortAddr      dw      ?               ;Base port for device
__IntNum        db      ?               ;Interrupt num for device
__DMAChan       db      ?               ;DMAChannel number for device
__DeviceRAM     dd      ?               ;Amount of ram for device
__CurrentModule dd      ?               ;Pointer to current playing module
__DMAFlag       db      1               ;Enable DMA xfers (1=active)
__MixingRate    dw      ?               ;Mixing speed
__Version       dw      ?               ;Version number of soundcard
__ChannelData   dd      ?               ;Pointer to channel data structure

H_ChannelData   dd      ?               ;handle for __ChannelData

_IntMask        db      ?               ;Interrupt mask flag (irq0-15)
_IntMask2       db      ?               ;Interrupt mask flag (irq0-15)
Old_Int_Handler dd      ?               ;Pointer to the irq handler
LoopJumpPoint   dd      ?               ;Pointer to song end handle code
LoopCount       db      ?               ;Counter for song loops

		even
		dw      1814
Period_Table    dw      1712,1616,1524,1440,1356,1280,1208 ;Period table
		dw      1140,1076,1016,0960,0907       ; for freq conversion

		; Gravis Volume table
Volume_Table    dw      0
		dw      1927*16,2243*16,2429*16,2561*16,2664*16,2748*16,2819*16,2880*16,2935*16,2983*16,3027*16
		dw      3067*16,3104*16,3138*16,3170*16,3200*16,3228*16,3254*16,3279*16,3303*16,3325*16,3347*16
		dw      3367*16,3387*16,3406*16,3424*16,3441*16,3458*16,3474*16,3490*16,3505*16,3520*16,3534*16
		dw      3548*16,3561*16,3574*16,3587*16,3599*16,3611*16,3623*16,3634*16,3645*16,3656*16,3667*16
		dw      3677*16,3687*16,3697*16,3707*16,3716*16,3726*16,3735*16,3744*16,3753*16,3761*16,3770*16
		dw      3778*16,3786*16,3794*16,3802*16,3810*16,3817*16,3825*16,3832*16,3840*16

		;Divisors for number of active GUS voices
Divisor_Table   dw      44100,41160,38587,36317,34300,32494,30870,29400
		dw      28063,26843,25725,24696,23746,22866,22050,21289
		dw      20580,19916,19293

Vibrato_Tables  dd      offset Vib_Sine_Table   ;Vibrato table 1 (sine)
		dd      offset Vib_Ramp_Table   ;Vibrato table 2 (ramp)
		dd      offset Vib_Pulse_Table  ;Vibrato table 3 (pulse)
		dd      offset Vib_Rand_Table   ;Vibrato table 4 (random)

Vib_Sine_Table  dw      0,25,50,74,98,120,142,162,180,197
		dw      212,225,236,244,250,254,255,254,250
		dw      244,236,225,212,197,180,162,142,120
		dw      98,74,50,25
		dw      -0,-25,-50,-74,-98,-120,-142,-162,-180,-197
		dw      -212,-225,-236,-244,-250,-254,-255,-254,-250
		dw      -244,-236,-225,-212,-197,-180,-162,-142,-120
		dw      -98,-74,-50,-25

Vib_Ramp_Table  dw      -255,-248,-240,-232,-224,-216,-208,-200,-192,-184,-176,-168
		dw      -160,-152,-144,-136,-128,-120,-112,-104,-96,-88,-80,-72,-64,-56
		dw      -48,-40,-32,-24,-16,-8
		dw      -0,16,24,32,40,48,56,64,72,80,88,96,104,112
		dw      120,128,136,144,152,160,168,176,184,192,200
		dw      208,216,224,232,240,248,255

Vib_Pulse_Table dw      0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0
		dw      0,0,0,0,0,0,0,0,0,0,0,0,0,0
		dw      255,255,255,255,255,255,255,255
		dw      255,255,255,255,255,255,255,255
		dw      255,255,255,255,255,255,255,255
		dw      255,255,255,255,255,255,255

Vib_Rand_Table  dw      105,17,40,-108,-102,140,-249,133,161,107,-233,-45,185,148,-65,236
		dw      190,-228,230,-70,12,136,-229,47,-17,-104,62,75,-121,-113,168,166
		dw      45,248,210,-140,99,245,-132,17,-202,255,90,-248,38,-205,-204,153
		dw      -111,-233,-105,-61,-102,229,245,-51,-114,-174,-173,75,-47,-45,108
		dw      -89,105,17,40,-108,-102,140,-249,133,161,107,-233,-45,185,148,-65
		dw      236,190,-228,230,-70,12,136,-229,47,-17,-104,62,75,-121,-113,168
		dw      166,45,248,210,-140,99,245,-132,17,-202,255,90,-248,38,-205,-204
		dw      153,-111,-233,-105,-61,-102,229,245,-51,-114,-174,-173,75,-47,-45,108,-89

FineTuneTable   dw      7895,7941,7985,8046,8107,8169,8232,8280
		dw      8363,8413,8463,8529,8581,8651,8723,8757

Retrig_Table    db      0,-1,-2,-4,-8,-16,0,0,0,1,2,4,8,16,0,0

;Local system variables
LoadPriority    db      ?               ;Load into memory: mode
S3M_ID          db      'SCRM'
MTM_ID          db      'MTM'

NOTE            db      255
INSTRUMENT      db      0
VOLUME          db      255
COMMAND         db      255
INFO            db      0

Temp_Counter    dd      ?               ;temp sample byte counter
Temp_Pointer    dd      ?               ;temp dram pointer
Temp_Mode       db      ?               ;temp gus mode val
Temp_Rows       db      ?               ;temp row counter (mtm converter)
Temp_MaxPat     dw      ?               ;temp maximum pat (mtm converter)
Temp_XOR        db      ?               ;temp xor value (mod&mtm converters)

_RealModeCode   db      21 dup(?)       ;Real mode irq handler
_OldRealMode    dd      ?               ;Old real mode irq handler


;--------------------------------
; MOD loading variables

IDCODES         LABEL DWORD
		db      'M.K.FLT4'      ;4 chn mod      ;ID codes for various mod types
		db      '6CHN'          ;6 chn mod
		db      '8CHNFLT8'      ;8 chn mod

inbuffer        dd      ?       ;buffers to hold files for processing
outbuffer       dd      ?
s3msamphdrs     dd      ?       ;buffer to hold sample headers

MTM_TrackPtr    dd      ?       ;temp pointer for mtm pattern conversion

insize          dd      ?       ;size of in and out files
outsize         dd      ?

CHANNELSETTINGS db      0,8,9,1,2,10,11,3       ;channel position values
		db      4,12,13,5,6,14,15,7
		db      0,8,9,1,2,10,11,3       ;and again for 32 chan mods
		db      4,12,13,5,6,14,15,7

;module conversion variables
NumberChans     db      ?
NumSamps        dw      31
CNumSamps       dw      ?
limitflag       dw      ?       ;flag for amiga flags active of not

_MSamNum        db      ?       ;pt values to convert
_MPtchNum       dw      ?
_MFxNum         db      ?
_MDataNum       db      ?
_SSamNum        db      ?       ;s3m values to write
_SPtchNum       db      ?
_SFxNum         db      ?
_SDataNum       db      ?
_SVolNum        db      ?

S3M_GV          equ     64      ;settings for s3m header
S3M_IS          equ     6       ;initial speed
S3M_GT          equ     125     ;initial tempo
S3M_MV          equ     176     ;master volume (sb only) & stereo flag
S3M_UC          equ     0       ;ultra click (n/a)
S3M_DP          equ     0       ;default pan (not required for mods)
S3M_DPYES       equ     252     ;default pan (required for mtms)
S3M_FLAGS       equ     0       ;16      ;Amiga Limits
S3M_CWTV        equ     1301h   ;Created with tracker version (emulate 3.01)
S3M_FFI         equ     2       ;file format information flag (2)

		;table to convert fine tune pitch values
C2SPD_Table     dd      8363,8413,8463,8529,8581,8651,8723,8757
		dd      7895,7941,7985,8046,8107,8169,8232,8280

PeriodVals      dw      856*4,808*4,762*4,720*4,678*4,640*4,604*4,570*4,538*4,508*4,480*4,453*4
		dw      856*2,808*2,762*2,720*2,678*2,640*2,604*2,570*2,538*2,508*2,480*2,453*2
		dw      856,808,762,720,678,640,604,570,538,508,480,453
		dw      428,404,381,360,339,320,302,285,269,254,240,226
		dw      214,202,190,180,170,160,151,143,135,127,120,113
		dw      214/2,202/2,190/2,180/2,170/2,160/2,151/2,143/2,135/2,127/2,120/2,113/2
		dw      214/4,202/4,190/4,180/4,170/4,160/4,151/4,143/4,135/4,127/4,120/4,113/4

FXConvTable     db      6       ;fx1=slide up
		db      5       ;fx2=slide dn
		db      7       ;fx3=porta
		db      8       ;fx4=vib
		db      12      ;fx5=porta+vslide
		db      11      ;fx6=vib+vslide
		db      18      ;fx7=tremolo
		db      24      ;fx8=PAN? (Xxx)
		db      15      ;fx9=samp offset
		db      4       ;fxa=vol slide
		db      2       ;fxb=posn jump
		db      0       ;fxc=set vol (N/A)
		db      3       ;fxd=pattn break
		db      0       ;fxe=extd cmds
		db      0       ;fxf=set speed (tempo)

_DATA           ends

;----------------------------------------------------------------------------
; CODE
;----------------------------------------------------------------------------
_TEXT           segment use32 dword public 'CODE'
		assume  cs:_TEXT,ds:_DATA,es:_DATA

;----------------------------------------------------------------------------
; Music Library Code Goes Here
;----------------------------------------------------------------------------
PM_SetLoopCode  proc near       ;Set address for code when module reaches end
				;EDX=Pointer to NEAR code32 routine
		mov [LoopJumpPoint],edx
		ret
PM_SetLoopCode  endp

PM_SetMasterVol proc near               ;Set vol of current module

		comment %

		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		mov ebx,[__CurrentModule]
		or ebx,ebx
		jz @@abort
		cmp al,0
		jg @@nofix
		mov al,0
@@nofix:        cmp al,64
		jb @@nofix2
		mov al,64
@@nofix2:       mov [ebx+_RealGlobalVol],al
		call SetAllVol
		clc
		ret
@@abort:        stc

		%

		ret
PM_SetMasterVol endp

PM_GetMasterVol proc near               ;Get vol of current module
		comment %

		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		mov ebx,[__CurrentModule]
		or ebx,ebx
		jz @@abort
		mov al,[ebx+_RealGlobalVol]
		call SetAllVol
		clc
		ret
@@abort:        stc

		%

		ret
PM_GetMasterVol endp

SetAllVol       proc near               ;Set all vols on active channels

		push edi
		mov edi,[__ChannelData]
		xor ecx,ecx
@@setloop:      or [edi+_ChannelFlag],_CHN_NewVol
		add edi,ChanDataSize
		inc cl
		cmp cl,[ebx+_TotalChanNum]
		jb @@setloop
		pop edi
		ret
SetAllVol       endp

;----------------------------------------------------------------------------
PM_ReleaseModule proc near              ;AL=non-zero = free sys-ram

		comment %

		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		cmp ebx,0
		jz @@abort
		cmp ebx,[__CurrentModule]
		jnz @@nostopit
		push eax ebx
		call PM_StopModule
		pop ebx eax
@@nostopit:     cmp [__SoundDevice],DEVICE_SBMONO
		jz @@freehere
		cmp [__SoundDevice],DEVICE_GUS
		jnz @@abort

		push eax                        ;Subtract old GUS samples
		cmp [ebx+_SampleFlag],0
		je @@nofree
		mov eax,[ebx+_SampleSize]
		sub [_DRAM_Counter],eax
		mov [ebx+_SampleFlag],0
@@nofree:       pop eax
@@freehere:     cmp al,0
		jz @@done                       ;See if free sys-mem aswell

		mov eax,[ebx+_Pointer]
		cmp eax,640*1024
		jb @@sublow
		mov eax,[ebx+_Size]
		sub [_himembase],eax
		jmp short @@donesub
@@sublow:       mov eax,[ebx+_Size]
		sub [_lomembase],eax
@@donesub:      push ebx
		mov ecx,M_Struc_Size
		mov al,0
@@loophere:     mov ds:[ebx],al
		inc ebx
		dec ecx
		jnz @@loophere
		pop ebx
@@done:         clc
		ret
@@abort:        stc

		%

		ret
PM_ReleaseModule endp

PM_GetDeviceRAM proc near

		comment %

		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		mov eax,[__DeviceRAM]
		cmp [__SoundDevice],DEVICE_GUS
		jnz @@trysb
		mov eax,[__DeviceRAM]
		sub eax,[_DRAM_Counter]
		clc
		ret
@@trysb:        cmp [__SoundDevice],DEVICE_SBMONO
		jnz @@abort
		call _lomemsize         ;get free ram
		mov edx,eax
		call _himemsize
		add eax,edx
		mov [__DeviceRAM],eax   ;set free ram size
		clc
		ret
@@abort:        stc

		%

		ret
PM_GetDeviceRAM endp

PM_PlayModule   proc near               ;Start song ptr EBX
		comment %

					;  AL=Forced Mixing volume (0 if none)
		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		or ebx,ebx
		jz @@abort
		test [__SystemStatus],SYS_PLAY
		jz @@playok
		push eax
		push ebx
		call PM_StopModule
		pop ebx
		pop eax
@@playok:       cmp [__SoundDevice],DEVICE_GUS
		jnz @@trysb
		call GUS_StartSong
		or [__SystemStatus],SYS_PLAY
		clc
		ret
@@trysb:        cmp [__SoundDevice],DEVICE_SBMONO
		jnz @@abort
		or al,al                        ;force mixing vol if reqd
		jz @@nochange
		mov ah,[ebx+_mastervol]
		and ax,1000000001111111b
		or al,ah
		mov [ebx+_mastervol],al
@@nochange:     call SB_StartSong
		or [__SystemStatus],SYS_PLAY
		clc
		ret
@@abort:        stc

		%

		ret
PM_PlayModule   endp

;----------------------------------------------------------------------------
PM_StopModule   proc near               ;Start song ptr EBX

		comment %

		cmp [__SystemStatus],SYS_UNINIT
		jz @@abort
		test [__SystemStatus],SYS_PLAY
		jz @@abort

		cmp [__CurrentModule],0
		jz @@abort
		cmp [__SoundDevice],DEVICE_GUS
		jnz @@trysb
		call GUS_StopSong
		jmp short @@stopped
@@trysb:        cmp [__SoundDevice],DEVICE_SBMONO
		jnz @@abort
		call SB_StopSong

@@stopped:      mov [__CurrentModule],0
		mov al,SYS_PLAY
		xor al,255
		and [__SystemStatus],al
		clc
		ret
@@abort:        stc

		%

		ret
PM_StopModule   endp

;----------------------------------------------------------------------------
PM_GetFileType  proc near       ;determine if file EDX is valid module
				;return AL=mod type
				; fills EDI=title of module (28 chars)

		comment %

		push edi
		call _openfile          ;Open file to read
		jc @@abort
		call GetFileType
		cmp al,0
		jz @@abortc

		pop edi
		push eax

		push edi eax
		mov ecx,28              ;blank title
		xor al,al
		rep stosb
		pop eax

		mov ecx,28
		cmp al,TYPE_S3M
		jz @@loadtitle
		mov ecx,20
		cmp al,TYPE_MOD
		jz @@loadtitle
		mov eax,04h
		mov bl,0
		call _lseekfile         ;seek to mtm title
@@loadtitle:    pop edx
		call _readfile
		call _closefile
		pop eax
		clc
		ret
@@abortc:       call _closefile
@@abort:        pop edi
		stc

		%

		ret

PM_GetFileType  endp

;----------------------------------------------------------------------------
PM_LoadModule   proc near       ;Load module at filename ptr EDX
				;Module header at EBX
				;AL=Loading parameters

		comment %

		mov esi,ebx
		mov [LoadPriority],al

		;Determine file exists + get size

		call _openfile          ;Open file to read
		jc @@abort
		call _filesize          ;Determine file size
		jnc @@sizefine
		jmp @@abortc
@@sizefine:     mov [esi+_Size],eax

		;determine file type (s3m or mod)
		push eax
		call GetFileType
		mov dl,al
		pop eax
		cmp dl,0
		jz @@abort
		cmp dl,TYPE_S3M
		jz @notamod

		call LoadModFile                ;convert mod/mtm to s3m format
		jc @@abort
		mov [esi+_Size],ebx
		mov [esi+_Pointer],eax
		jmp @@loadedfine

@notamod:       test [LoadPriority],LOAD_LO
		jnz @@ldlo
		test [LoadPriority],LOAD_HI
		jnz @@ldhi

		;Else load any mem
		call _getmem
		jc @@abortc
		jmp short @@loaddata
@@ldhi:         call _gethimem
		jc @@abortc
		jmp short @@loaddata
@@ldlo:         call _getlomem
		jc @@abortc
@@loaddata:     mov [esi+_Pointer],eax
		mov edx,eax
		mov ecx,[esi+_Size]

		push esi
		call _readfile          ;Load module
		pop esi

@@loadedfine:   push esi esi
		call _closefile
		pop esi
		call ParseModule
		pop esi

@@exit:         mov [esi+_SampleFlag],0
		test [LoadPriority],LOAD_DUMP   ;Check for sample dump
		jz @@quit
		test [__SoundDevice],DEVICE_GUS
		jz @@quit2

		mov eax,[esi+_SampleSize]       ;Ensure enuff DRAM 2 dump
		mov edx,[__DeviceRAM]
		sub edx,[_DRAM_Counter]
		cmp edx,eax
		jb @@quit

		push ebx
		mov ebx,[esi+_Pointer]          ;Dump samples to DRAM
		call G_DumpSams2DRAM
		pop ebx

@@quit2:        mov [esi+_SampleFlag],1

@@quit:         clc
		ret
@@abortc:       call _closefile
@@abort:        mov [esi+_Size],0
		stc

		%

		ret
PM_LoadModule   endp

comment %

GetFileType     proc near               ;quickly determine mod file type
		push ebx esi edi edx
		mov eax,02ch
		mov bl,ah
		call _lseekfile         ;seek to S3M id codes
		jc @@notas3m
		mov edx,offset LoaderCodes
		mov ecx,4
		call _readfile          ;read possible id code
		jc @@notas3m
		xor eax,eax
		mov bl,al
		call _lseekfile         ;seek to start

		mov esi,offset LoaderCodes ;Check for 'SCRM' in module
		mov edi,offset S3M_ID
		mov ecx,4
		rep cmpsb
		jnz @@notas3m
		mov al,TYPE_S3M         ;Set module type to s3m
		jmp @foundtype
@@notas3m:
		mov eax,1080
		mov bl,0
		call _lseekfile         ;seek to MOD id codes
		jc @notamod
		mov edx,offset LoaderCodes
		mov ecx,4
		call _readfile          ;read possible id code
		jc @notamod
		xor eax,eax
		mov bl,al
		call _lseekfile         ;seek to start

		mov ebx,offset IDCODES
		mov edx,offset LoaderCodes
		call GetNumChans
		jc @notmod
		mov al,TYPE_MOD         ;Set module type to mod
		jmp short @foundtype
@notmod:
		xor eax,eax             ;check for MTM
		mov bl,al
		call _lseekfile         ;seek to start
		mov edx,offset LoaderCodes
		mov ecx,4
		call _readfile          ;read possible id code
		jc @@notamtm
		xor eax,eax
		mov bl,al
		call _lseekfile         ;seek to start

		mov esi,offset LoaderCodes ;Check for 'MTM' in module
		mov edi,offset MTM_ID
		mov ecx,3
		rep cmpsb
		jnz @@notamtm
		mov al,TYPE_MTM         ;Set module type to mtm
		jmp @foundtype

@@notamtm:      mov al,0
@foundtype:     pop edx edi esi ebx
		ret
GetFileType     endp

LoadModFile     proc near               ;convert mod file to s3m
		mov eax,[esi+_Size]
		pushad
		mov [insize],eax
		call _gethimem          ;allocate out buffer first
		jc @abort
		mov [outbuffer],eax
		mov eax,[insize]
		call _gethimem
		jc @abort2
		mov [inbuffer],eax
		push edx
		mov edx,eax
		mov ecx,[insize]
		call _readfile          ;Load module
		pop edx

		cmp dl,TYPE_MTM
		jnz @@typeismod
		call ConvertMTM
		jc @abort3
		jmp short @@skipmodl
@@typeismod:    call ConvertMOD
		jc @abort3

@@skipmodl:     movzx eax,[NumSamps]    ;free ram used while converting
		mov bx,80
		mul bx                  ;  free temp buffers
		sub [_himembase],eax
		mov eax,[insize]
		sub [_himembase],eax
		sub eax,[outsize]       ;  free loaded mod
		sub [_himembase],eax

		popad
		mov eax,[outbuffer]
		mov ebx,[outsize]
		clc
		ret

@abort:         stc
		popad
		ret
@abort2:        mov eax,[insize]
		sub [_himembase],eax
		jmp @abort
@abort3:        mov eax,[insize]
		sub [_himembase],eax
		jmp @abort2

LoadModFile     endp

;----------------------------------------------------------------------------
;----------------------------------------------------------------------------
; MOD conversion code
;----------------------------------------------------------------------------
ConvertMOD      proc near       ;main conversion routine, converts INBUFFER to OUTBUFFER
		mov esi,[inbuffer]
		mov edi,[outbuffer]

		mov ecx,[insize]        ;clear target buffer
		push edi
		mov al,0
		rep stosb
		pop edi

		mov ebx,offset IDCODES
		lea edx,[esi+1080]
		call GetNumChans                ;first determine MOD type
		jc @@abort

		;start preparing s3m header

		push esi edi                    ;copy module title to s3m header
		mov cl,20
@cpytitle:      movsb
		dec cl
		jnz @cpytitle
		pop edi esi

		call SetStartHeader

		call SetChannels        ;write channel values

		movzx eax,[NumSamps]    ;allocate s3m sample header buffer
		mov bx,80
		mul bx
		call _gethimem
		mov [s3msamphdrs],eax
		push esi edi
		add esi,20
		mov edi,eax
		call ConvertSamps
		pop edi esi

		movzx eax,byte ptr [esi+950] ;get number of patterns and list length
		mov [edi+20h],ax
		mov cl,al
		mov ah,al
		push edi esi
		add edi,60h
		add esi,952
@nextone:       mov al,[esi]
		inc esi
		mov [edi],al
		inc edi
		dec cl
		jnz @nextone
		mov al,0ffh
		shr ah,1
		jc @odd
		stosb
@odd:           stosb
		pop esi edi

		push esi                ;determine last pattern in list
		;mov cl,[esi+950]
		mov cl,128
		add esi,952
		xor eax,eax
@nextbyte:      mov al,[esi]
		cmp al,255
		jz @nobigger
		cmp al,ah
		jb @nobigger
		mov ah,al
@nobigger:      inc esi
		dec cl
		jnz @nextbyte
		pop esi
		inc ah
		mov al,ah
		movzx eax,al
		mov [edi+24h],ax        ;store value in s3m header
		mov ax,[CNumSamps]
		mov word ptr [edi+22h],ax ;store number of insts

		;copy sample headers
		xor eax,eax
		mov al,60h              ;calc offset for start of headers
		add ax,[edi+20h]
		mov ebx,eax
		add ebx,edi             ;ebx=ptr to sample offset table
		add ax,[edi+22h]
		add ax,[edi+22h]
		add ax,[edi+24h]
		add ax,[edi+24h]
		test eax,0fh            ;adjust for page boundaries
		jz @noinctopage
		shr eax,4
		inc eax
		shl eax,4

@noinctopage:   mov ebp,eax             ;ebp=counter for offset in file
		push edi
		add edi,eax             ;edi=dest for sample header
		push esi
		mov esi,[s3msamphdrs]
		mov cx,[CNumSamps]
		shl cx,8
@nexthdr:       mov eax,ebp
		shr eax,4
		mov [ebx],ax            ;store in inst defn list
		add ebx,2
		mov cl,80               ;move the header
@movenextb:     movsb
		dec cl
		jnz @movenextb
		add ebp,80
		dec ch                  ;copy all headers
		jnz @nexthdr
		mov [outsize],ebp
		pop esi
		pop edi

		;convert mod patterns to s3m format
		mov edx,60h             ;calc offset for start of headers
		add dx,[edi+20h]
		add dx,[edi+22h]
		add dx,[edi+22h]
		add edx,edi
		;ebp=offset in file for writing + tables
		;edx=ptr to current pattern offset in table
		;esi=ptr to start of mod file
		;edi=ptr to start of s3m file (being created)

		mov [limitflag],16              ;default=amiga limits on
		call ConvertPatterns

		mov ax,[limitflag]              ;set limits if all notes within amiga limits
		mov word ptr [edi+26h],ax

		;copy samples to s3m format
		xor edx,edx
		mov dl,60h              ;calc offset for start of headers
		add dx,[edi+20h]
		add edx,edi
		mov [Temp_XOR],128
		call CopySamples

@@abort:        ret
ConvertMOD      endp

;----------------------------------------------------------------------------
; MTM conversion code
;----------------------------------------------------------------------------
ConvertMTM      proc near

		mov esi,[inbuffer]
		mov edi,[outbuffer]

		mov ecx,[insize]        ;clear target buffer
		push edi
		mov al,0
		rep stosb
		pop edi

		mov al,[esi+33]         ;get num of voices
		or al,al
		jz @@abort
		cmp al,32               ;  check valid voice range
		ja @@abort
		mov [NumberChans],al

		movzx eax,byte ptr [esi+30] ;get num of samples
		mov [NumSamps],ax

		;start preparing s3m header

		push esi edi            ;copy module title to s3m header
		add esi,4
		mov cl,20
@@cpytitle:     movsb
		dec cl
		jnz @@cpytitle
		pop edi esi

		call SetStartHeader
		mov byte ptr [edi+35h],S3M_DPYES ;default pan included

		call SetChannels        ;write channel values

		movzx eax,[NumSamps]    ;allocate s3m sample header buffer
		mov bx,80
		mul bx
		call _gethimem
		mov [s3msamphdrs],eax   ;convert sample headers
		push esi edi
		add esi,66
		mov edi,eax
		call ConvertMTMSamps
		pop edi esi

		movzx eax,byte ptr [esi+27] ;get number of patterns and list length
		inc eax
		mov [edi+20h],ax

		mov cl,al               ;copy order list
		mov ah,al
		push edi esi
		add edi,60h
		add esi,66
		mov eax,37
		mul [NumSamps]
		add esi,eax
@@nextone:      movsb
		dec cl
		jnz @@nextone
		mov al,0ffh
		shr ah,1
		jc @@odd
		stosb
@@odd:          stosb
		pop esi edi

		movzx eax,byte ptr [esi+26] ;determine last pattern in list
		inc eax
		mov [edi+24h],ax        ;store value in s3m header
		mov ax,[CNumSamps]
		mov word ptr [edi+22h],ax ;store number of insts

		;copy sample headers
		mov eax,60h             ;calc offset for start of headers
		add ax,[edi+20h]
		mov ebx,eax
		add ebx,edi             ;ebx=ptr to sample offset table
		add ax,[edi+22h]
		add ax,[edi+22h]
		add ax,[edi+24h]
		add ax,[edi+24h]

		push eax esi edi        ;set mtm default pan settings
		add edi,eax
		add esi,34
		mov ecx,32
@@setploop:     lodsb
		or al,00100000b
		stosb
		loop @@setploop
		pop edi esi eax
		add eax,32

		test eax,0fh            ;adjust for page boundaries
		jz @@noinctopage
		shr eax,4
		inc eax
		shl eax,4

@@noinctopage:  mov ebp,eax             ;ebp=counter for offset in file
		push edi
		add edi,eax             ;edi=dest for sample header
		push esi
		mov esi,[s3msamphdrs]
		mov cx,[CNumSamps]
		shl cx,8
@@nexthdr:      mov eax,ebp
		shr eax,4
		mov [ebx],ax            ;store in inst defn list
		add ebx,2
		mov cl,80               ;move the header
@@movenextb:    movsb
		dec cl
		jnz @@movenextb
		add ebp,80
		dec ch                  ;copy all headers
		jnz @@nexthdr
		mov [outsize],ebp
		pop esi
		pop edi

		movzx eax,word ptr [esi+28]
		push eax

		;convert mtm patterns to s3m format
		mov edx,60h             ;calc offset for start of headers
		add dx,[edi+20h]
		add dx,[edi+22h]
		add dx,[edi+22h]
		add edx,edi
		;ebp=offset in file for writing + tables
		;edx=ptr to current pattern offset in table
		;esi=ptr to start of mtm file
		;edi=ptr to start of s3m file (being created)

		mov [limitflag],16              ;default=amiga limits on
		call ConvertMTMPats

		mov ax,[limitflag]              ;set limits if all notes within amiga limits
		mov word ptr [edi+26h],ax

		;copy samples to s3m format
		;mov ax,[NumSamps]
		;mov [CNumSamps],ax

		;copy samples to s3m format
		mov edx,60h             ;calc offset for start of headers
		add dx,[edi+20h]
		add edx,edi
		mov [Temp_XOR],0
		pop eax
		add esi,eax
		call CopySamples

@@abort:        ret
ConvertMTM      endp

ConvertMTMPats  proc near               ;convert mtm pattern to s3m pattern
		push edi

		push edx
		movzx ecx,word ptr [edi+24h]     ;cx=count for number of patterns to convert
		mov [Temp_MaxPat],cx
		mov eax,37
		mul [NumSamps]  ;byte ptr [esi+30]
		add eax,194
		add eax,esi
		mov [MTM_TrackPtr],eax  ;get ptr to all tracks
		
		mov eax,192
		mul word ptr [esi+24]
		add eax,[MTM_TrackPtr]  ;esi=ptr to current pattern to convert
		mov esi,eax             ;set esi to track channel list
		pop edx

		add edi,[outsize] ;edi=ptr to current pattern s3m destination

@@convertloop:  mov [outsize],ebp
		mov eax,ebp     ;set pointer to patt in inst table
		shr eax,4
		mov [edx],ax
		add edx,2

		mov ebx,edi     ;keep first address for size value
		call ConvertItMTM
		mov eax,ebp     ;write packed pattern size
		sub eax,[outsize]
		mov [ebx],ax

		and al,0fh      ;adjust for next page
		mov ah,16
		sub ah,al
		mov al,ah
		and eax,0fh
		add edi,eax
		add ebp,eax
		dec ecx         ;convert all patterns
		jnz @@convertloop

		mov [outsize],ebp
		pop edi
		ret
ConvertMTMPats  endp

ConvertItMTM    proc near       ;convert MTM pattern data
		push ebx ecx edx
		add edi,2       ;skip first 2 length bytes
		add ebp,2

		mov [Temp_Rows],0 ;convert patterns loop
@@nextrow:      xor ecx,ecx
		push esi
@@nextchan:     push esi
		
		movzx eax,word ptr [esi] ;get ptr to correct track
		or eax,eax
		jz @@blankpat           ;check for zero pattern
		;cmp ax,[Temp_MaxPat]
		;ja @@blankpat
		dec eax
		mov ebx,192
		mul ebx
		mov esi,[MTM_TrackPtr]

		add esi,eax

		mov eax,3
		mul [Temp_Rows]
		add esi,eax

		push ecx
		call ConvertNoteMTM
		pop ecx

@@blankpat:     pop esi
		inc esi
		inc cl
		inc esi
		cmp cl,32       ;[NumberChans]
		jb @@nextchan

		pop esi

		mov byte ptr [edi],0    ;set zero to mark end of row
		inc edi
		inc ebp

		inc [Temp_Rows]
		cmp [Temp_Rows],64
		jb @@nextrow
		add esi,32*2
		pop edx ecx ebx
		ret
ConvertItMTM    endp

ConvertNoteMTM  proc near

		movzx eax,byte ptr [esi]
		shl eax,16
		mov ah,[esi+1]
		mov al,[esi+2]

		or eax,eax              ;check if no note first
		jz @@donecon

		mov [edi],cl    ;first set channel number

		mov [_SSamNum],0        ;blank settings
		mov [_SPtchNum],255
		mov [_SVolNum],255
		mov [_SFxNum],255
		mov [_SDataNum],0

		mov ah,[esi]
		mov al,[esi+1]
		shr eax,4
		and al,00111111b ;al=sample number
		mov [_MSamNum],al

		movzx eax,byte ptr [esi]
		shr al,2
		or eax,eax
		jz @@nonote             ;calculate octave + note
		push ebx
		mov bl,12
		div bl
		pop ebx
		add al,2
		shl al,4
		or al,ah
		mov [_SPtchNum],al
@@nonote:       movzx eax,byte ptr [_SPtchNum]
		push eax
		mov [_MPtchNum],0       ;dummy value - not used
		cmp al,255
		jz @@nopitch
		mov [_MPtchNum],1712    ;dummy value - not used
@@nopitch:      mov al,[esi+1]
		and al,0fh
		mov [_MFxNum],al        ;pt fx number
		mov al,[esi+2]
		mov [_MDataNum],al      ;fx datavalue
		call ConvertValues
		pop eax
		mov [_SPtchNum],al

		mov ebx,edi
		inc edi
		inc ebp
		cmp [_SPtchNum],255
		jnz @@yepnote
		cmp [_SSamNum],0
		jz @@nopenote
@@yepnote:      or byte ptr [ebx],32    ;set new note/sample
		mov al,[_SPtchNum]
		mov [edi],al
		inc edi
		inc ebp
		mov al,[_SSamNum]
		mov [edi],al
		inc edi
		inc ebp
@@nopenote:     cmp [_SVolNum],255
		jz @@novol
		or byte ptr [ebx],64    ;set new volume
		mov al,[_SVolNum]
		mov [edi],al
		inc edi
		inc ebp
@@novol:        cmp [_SFxNum],255
		jz @@donecon
		or byte ptr [ebx],128   ;set new fx
		mov al,[_SFxNum]
		mov [edi],al
		inc edi
		inc ebp
		mov al,[_SDataNum]
		mov [edi],al
		inc edi
		inc ebp
@@donecon:      ret
ConvertNoteMTM  endp

;----------------------------------------------------------------------------
SetStartHeader  proc near               ;create part of s3m header
		mov byte ptr [edi+1ch],01ah     ;eof after title
		mov byte ptr [edi+1dh],16       ;[Typ] value (16)
		mov byte ptr [edi+2ch],'S'      ;create 'SCRM' id code
		mov byte ptr [edi+2dh],'C'
		mov byte ptr [edi+2eh],'R'
		mov byte ptr [edi+2fh],'M'
		mov byte ptr [edi+30h],S3M_GV   ;global volume
		mov byte ptr [edi+31h],S3M_IS   ;initial speed
		mov byte ptr [edi+32h],S3M_GT   ;initial tempo
		mov byte ptr [edi+33h],S3M_MV   ;master volume
		mov byte ptr [edi+34h],S3M_UC   ;ultra click (n/a)
		mov byte ptr [edi+35h],S3M_DP   ;default pan
		;mov word ptr [edi+26h],S3M_FLAGS ;Amiga Limits
		mov word ptr [edi+28h],S3M_CWTV ;CWT version
		mov word ptr [edi+2ah],S3M_FFI  ;FFI flag
		ret
SetStartHeader  endp

;----------------------------------------------------------------------------
CopySamples     proc near               ;copy samples from mod to s3m
		mov ebp,[outsize]

		movzx ecx,word ptr [edi+22h]
@@copyloop:     push ecx
		movzx ebx,word ptr [edx] ;get ptr to sample header
		add edx,2
		shl ebx,4
		add ebx,edi

		mov ecx,[ebx+10h]       ;write offset to header
		or ecx,ecx
		jz @nocopy

		mov eax,ebp
		shr eax,4
		mov [ebx+0eh],ax

@@cpyb:         lodsb            ;copy bytes and increase pointers
		xor al,[Temp_XOR]       ;128
		mov [edi+ebp],al
		inc ebp
		dec ecx
		jnz @@cpyb

		mov eax,ebp
		sub eax,[outsize]
		and al,0fh      ;adjust for next page
		mov ah,16
		sub ah,al
		mov al,ah
		and eax,0fh
		add ebp,eax
		mov [outsize],ebp
@nocopy:        pop ecx
		dec ecx
		jnz @@copyloop
		ret
CopySamples     endp

ConvertPatterns proc near       ;convert all the patterns to s3m format
		push edi
		movzx ecx,word ptr [edi+24h] ;cx=count for number of patterns to convert
		add esi,1084    ;esi=ptr to current pattern to convert
		add edi,[outsize] ;edi=ptr to current pattern s3m destination

@convertloop:   mov [outsize],ebp
		mov eax,ebp     ;set pointer to patt in inst table
		shr eax,4
		mov [edx],ax
		add edx,2

		mov ebx,edi     ;keep first address for size value
		call ConvertIt
		mov eax,ebp     ;write packed pattern size
		sub eax,[outsize]
		mov [ebx],ax

		and al,0fh      ;adjust for next page
		mov ah,16
		sub ah,al
		mov al,ah
		and eax,0fh
		add edi,eax
		add ebp,eax
		dec ecx          ;convert all patterns
		jnz @convertloop

		mov [outsize],ebp
		pop edi
		ret
ConvertPatterns endp

ConvertIt       proc near
		push ebx ecx edx
		add edi,2       ;skip first 2 length bytes
		add ebp,2

		mov ecx,64               ;convert patterns loop
@nextrow:       push ecx
		xor ecx,ecx
@nextchan:      call ConvertNote
		add esi,4

		inc cl
		cmp cl,[NumberChans]
		jb @nextchan

		mov byte ptr [edi],0    ;set zero to mark end of row
		inc edi
		inc ebp

		pop ecx
		dec ecx
		jnz @nextrow

		pop edx ecx ebx
		ret
ConvertIt       endp

ConvertNote     proc near       ;convert mod note to s3m note
		cmp dword ptr [esi],0   ;check if no note first
		jz @donecon

		mov [edi],cl    ;first set channel number

		mov [_SSamNum],0        ;blank settings
		mov [_SPtchNum],255
		mov [_SVolNum],255
		mov [_SFxNum],255
		mov [_SDataNum],0

		mov ah,[esi]
		shr ah,4
		mov al,[esi+2]
		shr eax,4        ;al=sample number
		mov [_MSamNum],al

		mov ax,[esi]
		xchg ah,al
		and ah,0fh      ;and ax,0fffh
		mov [_MPtchNum],ax ;pitch period

		mov al,[esi+2]
		and al,0fh
		mov [_MFxNum],al ;pt fx number
		mov al,[esi+3]
		mov [_MDataNum],al ;fx datavalue
		call ConvertValues

		mov ebx,edi
		inc edi
		inc ebp
		cmp [_SPtchNum],255
		jnz @yepnote
		cmp [_SSamNum],0
		jz @nopenote
@yepnote:       or byte ptr [ebx],32    ;set new note/sample
		mov al,[_SPtchNum]
		mov [edi],al
		inc edi
		inc ebp
		mov al,[_SSamNum]
		mov [edi],al
		inc edi
		inc ebp
@nopenote:      cmp [_SVolNum],255
		jz @novol
		or byte ptr [ebx],64    ;set new volume
		mov al,[_SVolNum]
		mov [edi],al
		inc edi
		inc ebp
@novol:         cmp [_SFxNum],255
		jz @nofx
		or byte ptr [ebx],128   ;set new fx
		mov al,[_SFxNum]
		mov [edi],al
		inc edi
		inc ebp
		mov al,[_SDataNum]
		mov [edi],al
		inc edi
		inc ebp
@nofx:
@donecon:       ret
ConvertNote     endp

ConvertValues   proc near       ;convert extracted pt values to s3m ones
		mov al,[_MSamNum]       ;copy sample number
		mov [_SSamNum],al

		cmp [_MPtchNum],0
		jz @nopitch
		push esi ecx ebx
		mov esi,offset PeriodVals
		mov ecx,7*12
		mov bx,0100h            ;ah=oct al=note
		mov ax,[_MPtchNum]

@trynext:       cmp [esi],ax            ;look up pitch on table
		jbe @foundptch  ;jz
		add esi,2
		dec ecx                  ;check for end of table
		jz @nofoundptch
		inc bl                  ;increse if not found
		cmp bl,12
		jb @trynext
		inc bh
		mov bl,0
		jmp @trynext
@nofoundptch:   xor ebx,ebx
@foundptch:     mov ax,bx
		cmp bh,3                ;check within limits and set
		jb @zroamiflg           ;  amiga flag accordingly
		cmp bh,6
		jb @nozroaflag
@zroamiflg:     mov [limitflag],0
@nozroaflag:    pop ebx ecx esi
		shl ah,4
		or al,ah
		or al,al
		jz @nopitch
		mov [_SPtchNum],al
@nopitch:       ;convert fx
		cmp [_MFxNum],0         ;check for arpeggio exception
		jnz @convfx
		cmp [_MDataNum],0
		jz @convertedall
		mov [_SFxNum],10        ;  set arpeggio
		mov al,[_MDataNum]
		mov [_SDataNum],al
		jmp @convertedall
@convfx:        movzx eax,byte ptr [_MFxNum]
		cmp al,0ch
		jnz @notvol
		mov al,[_MDataNum]      ;handle fx0C = set volume
		mov [_SVolNum],al
		jmp @convertedall
@notvol:        cmp al,0eh
		jb @notextd
		;handle extd commands and tempo
		cmp al,0fh
		jnz @ecmd
		mov al,[_MDataNum]      ;<--- add extd tempo flag check here
		mov [_SDataNum],al
		cmp al,32
		jbe @speed
		mov [_SFxNum],20                ;set bpm
		jmp @convertedall
@speed:         mov [_SFxNum],1                 ;set speed
		jmp @convertedall
@ecmd:          call ConvertECmd
		jmp @convertedall
@notextd:       dec al
		mov ebx,offset FXConvTable
		add ebx,eax
		mov al,[ebx]
		mov [_SFxNum],al
		mov al,[_MDataNum]
		mov [_SDataNum],al
@convertedall:  ret
ConvertValues   endp

EFXJumpTable    dd      offset _E_Same          ;0
		dd      offset _E_FinePup       ;1
		dd      offset _E_FinePdn       ;2
		dd      offset _E_GlissCtrl     ;3
		dd      offset _E_VibCtrl       ;4
		dd      offset _E_FineTune      ;5
		dd      offset _E_PattLp        ;6
		dd      offset _E_TremCtrl      ;7
		dd      offset _E_Same          ;8
		dd      offset _E_Retrig        ;9
		dd      offset _E_FineVup       ;a
		dd      offset _E_FineVdn       ;b
		dd      offset _E_Same          ;c
		dd      offset _E_Same          ;d
		dd      offset _E_Same          ;e
		dd      offset _E_Same          ;f

ConvertECmd     proc near       ;convert mod Exx command to s3m Sxx command
		movzx eax,byte ptr [_MDataNum]
		mov bl,al
		shr al,4
		call [EFXJumpTable+eax*4]    ;Jump to command
@doneecmd:      ret
ConvertECmd     endp

_E_Same:        mov [_SDataNum],bl              ;copy values exactly
		mov [_SFxNum],19
@retrn:         ret
_E_FinePup:     and bl,0fh                      ;fine slide pitch up
		or bl,bl
		jz @retrn
		or bl,0f0h
		mov [_SDataNum],bl
		mov [_SFxNum],6
		ret
_E_FinePdn:     and bl,0fh                      ;fine slide pitch dn
		or bl,bl
		jz @retrn
		or bl,0f0h
		mov [_SDataNum],bl
		mov [_SFxNum],5
		ret
_E_GlissCtrl:   and bl,0fh                      ;glissando control
		or bl,010h
		mov [_SDataNum],bl
		mov [_SFxNum],19
		ret
_E_VibCtrl:     and bl,0fh                      ;vibrato control
		or bl,030h
		mov [_SDataNum],bl
		mov [_SFxNum],19
		ret
_E_FineTune:    and bl,0fh                      ;set fine tune
		or bl,020h
		mov [_SDataNum],bl
		mov [_SFxNum],19
		ret
_E_PattLp:      and bl,0fh                      ;pattern loop
		or bl,0b0h
		mov [_SDataNum],bl
		mov [_SFxNum],19
		ret
_E_TremCtrl:    and bl,0fh                      ;tremolo control
		or bl,040h
		mov [_SDataNum],bl
		mov [_SFxNum],19
		ret
_E_Retrig:      and bl,0fh                      ;retrig
		mov [_SDataNum],bl
		mov [_SFxNum],17
		ret
_E_FineVup:     shl bl,4                        ;fine volume up
		or bl,bl
		jz @retrn2
		or bl,0fh
		mov [_SDataNum],bl
		mov [_SFxNum],4
		ret
_E_FineVdn:     and bl,0fh                      ;fine volume dn
		or bl,bl
		jz @retrn2
		or bl,0f0h
		mov [_SDataNum],bl
		mov [_SFxNum],4
@retrn2:        ret

SetChannels     proc near       ;set channel values in header
		push edi
		add edi,40h
		push edi
		mov al,255
		mov cl,32
@@next:         mov [edi],al    ;first make all = 255
		inc edi
		dec cl
		jnz @@next
		pop edi

		;set channel values according to mod position

		push esi
		mov esi,offset CHANNELSETTINGS
		mov cl,[NumberChans]
@@nextcpy:      movsb
		dec cl
		jnz @@nextcpy
		pop esi
		pop edi
		ret
SetChannels     endp

ConvertSamps    proc near       ;convert sample headers
		movzx ecx,word ptr [NumSamps]
@@convloop:     push ecx
		xor ebx,ebx             ;blank s3m samp header first
		xor eax,eax
@clrsamp:       mov [edi+ebx],al
		inc bl
		cmp bl,80
		jb @clrsamp

		xor ebx,ebx             ;copy title of sample
@nexttitl:      mov al,[esi+ebx]
		mov [edi+ebx+30h],al
		inc bl
		cmp bl,22
		jb @nexttitl

		movzx eax,word ptr [esi+22] ;determine sample length
		xchg ah,al
		shl eax,1
		mov [edi+10h],eax
		or eax,eax
		jz @getvol
		mov byte ptr [edi],1    ;set sample present flag
		mov byte ptr [edi+04ch],'S' ;write sample id code
		mov byte ptr [edi+04dh],'C'
		mov byte ptr [edi+04eh],'R'
		mov byte ptr [edi+04fh],'S'

@getvol:        mov al,[esi+25]         ;determine sample vol
		cmp al,64
		jbe @noclip
		mov al,64
@noclip:        mov [edi+1ch],al

		movzx eax,word ptr [esi+26] ;determine loop start point
		xchg ah,al
		shl eax,1
		cmp eax,[edi+10h]       ;  check not > than samp length
		jbe @noclip2
		mov eax,[edi+10h]
@noclip2:       mov [edi+14h],eax

		movzx eax,word ptr [esi+28] ;determine loop end point
		xchg ah,al
		shl eax,1
		cmp eax,4               ;check for loop too small
		jbe @noclip3
		mov byte ptr [edi+1fh],1 ;set loop enable flag
		add eax,[edi+14h]
		cmp eax,[edi+10h]
		jbe @noclip3
		mov eax,[edi+10h]
@noclip3:       mov [edi+18h],eax

		movzx eax,byte ptr [esi+24] ;determine C2SPD value
		and al,0fh
		mov eax,[C2SPD_Table+eax*4]
		mov [edi+20h],eax

		pop ecx                  ;loop to convert all samples
		;determine if filled samp
		cmp dword ptr [edi+10h],0
		jnz @samppresent
		cmp byte ptr [edi+30h],0
		jz @nosamppresent

@samppresent:   movzx eax,[NumSamps]
		inc eax
		sub eax,ecx
		mov [CNumSamps],ax

@nosamppresent: add esi,30
		add edi,80
		dec ecx
		jnz @@convloop
		ret
ConvertSamps    endp

ConvertMTMSamps proc near       ;convert sample headers for MTM module
		movzx ecx,[NumSamps]

@@convloop:     push ecx
		xor ebx,ebx             ;blank s3m samp header first
		xor eax,eax
@@clrsamp:      mov [edi+ebx],al
		inc bl
		cmp bl,80
		jb @@clrsamp

		xor ebx,ebx             ;copy title of sample
@@nexttitl:     mov al,[esi+ebx]
		mov [edi+ebx+30h],al
		inc bl
		cmp bl,22
		jb @@nexttitl

		mov eax,[esi+22]        ;get sample length in bytes
		mov [edi+10h],eax

		or eax,eax
		jz @@getvol
		mov byte ptr [edi],1    ;set sample present flag
		mov byte ptr [edi+04ch],'S' ;write sample id code
		mov byte ptr [edi+04dh],'C'
		mov byte ptr [edi+04eh],'R'
		mov byte ptr [edi+04fh],'S'

@@getvol:       mov al,[esi+35]         ;determine sample vol
		cmp al,64
		jbe @@noclip
		mov al,64
@@noclip:       mov [edi+1ch],al

		mov eax,[esi+26]        ;get loop start point
		cmp eax,[edi+10h]       ;  check not > than samp length
		jbe @@noclip2
		mov eax,[edi+10h]
@@noclip2:      mov [edi+14h],eax

		mov eax,[esi+30]        ;determine loop end point
		mov [edi+18h],eax
		sub eax,[edi+14h]
		cmp eax,4               ;check for loop too small
		jle @@noclip3
		mov byte ptr [edi+1fh],1 ;set loop enable flag
@@noclip3:
		movzx eax,byte ptr [esi+34] ;determine C2SPD value
		and al,0fh
		mov eax,[C2SPD_Table+eax*4]
		mov [edi+20h],eax

		mov al,[esi+36]         ;check for 16-bit sample
		and al,1                ;  (note: not supported in player yet)
		or al,al
		jz @@not16bit
		or byte ptr [edi+1fh],4 ;set 16-bit sample flag

@@not16bit:     pop ecx                 ;loop to convert all samples
		;determine if filled samp
		cmp dword ptr [edi+10h],0
		jnz @@samppresent
		cmp byte ptr [edi+30h],0
		jz @@nosamppresent

@@samppresent:  movzx eax,[NumSamps]
		inc eax
		sub eax,ecx
		mov [CNumSamps],ax

@@nosamppresent: add esi,37
		add edi,80
		dec ecx
		jnz @@convloop
		ret
ConvertMTMSamps endp

GetNumChans     proc near       ;determine number of channels + mod type
		mov [NumberChans],4
		call CmpIdCode          ;check for 4chn id codes
		jnc @@found
		add ebx,4
		call CmpIdCode
		jnc @@found
		add ebx,4
		mov [NumberChans],6     ;check for 6chn id codes
		call CmpIdCode
		jnc @@found
		add ebx,4
		mov [NumberChans],8     ;check for 8chn id codes
		call CmpIdCode
		jnc @@found
		add ebx,4
		call CmpIdCode
		jnc @@found

		cmp byte ptr [edx+2],'C'        ;check for 'CH'
		jnz @@abort
		cmp byte ptr [edx+3],'H'
		jnz @@abort
		sub byte ptr [edx],'0'          ;must be multi-channel mods (??CH)
		sub byte ptr [edx+1],'0'        ;calc numchans
		mov al,10
		mul byte ptr [edx]
		add al,[edx+1]
		add byte ptr [edx],'0'
		add byte ptr [edx+1],'0'
		cmp al,32                       ;no more than 32
		jbe @@validchans
		mov al,32
@@validchans:   mov [NumberChans],al
		clc
@@found:        mov [NumSamps],31
		ret
@@abort:        stc
		ret
GetNumChans     endp

CmpIdCode       proc near               ;compare 4 byte id codes
		push ebx edx ecx
		mov cl,4
@trynext1:      mov al,[ebx]            ;scan each byte
		cmp al,[edx]
		jnz @nope
		inc ebx
		inc edx
		dec cl
		jnz @trynext1
		pop ecx edx ebx
		clc
		ret
@nope:          pop ecx edx ebx
		stc
		ret
CmpIdCode       endp

;----------------------------------------------------------------------------
;----------------------------------------------------------------------------

ParseModule     proc near               ;Procedure to exract module info + initialise variables
					;ESI=pointer to module header
		cld
		mov ebp,esi
		mov edi,esi
		add edi,_Title
		mov esi,[esi+_Pointer]  ;ESI=pointer to module, EDI=header
		mov ecx,28/4
		rep movsd               ;Copy title to header info
		mov byte ptr [edi],0

		;Detect Module type

		mov esi,[ebp+_Pointer]  ;Check for 'SCRM' in module
		mov edi,offset S3M_ID
		add esi,44
		mov ecx,4
		rep cmpsb
		jz @@tis_ok
		jmp @@abort
@@tis_ok:       mov ax,TYPE_S3M         ;Set module type to s3m
		mov edi,ebp
		mov [edi+_Type],ax

		mov esi,[edi+_Pointer]  ;Copy important variables
		mov ax,[esi+20h]
		mov [edi+_Ordnum],ax
		mov ax,[esi+22h]
		mov [edi+_Insnum],ax
		mov ax,[esi+24h]
		mov [edi+_Patnum],ax

		mov al,[esi+30h]
		mov [edi+_globalvol],al
		mov al,[esi+31h]
		mov [edi+_initialspd],al
		mov al,[esi+32h]
		mov [edi+_initialBPM],al
		mov al,[esi+33h]
		mov [edi+_mastervol],al
		shr al,7                        ;set stereo flag
		mov [edi+_stereoflag],al

		mov ax,[esi+026h]               ;General module flags
		mov [edi+_generalflags],ax

		add esi,40h             ;Determine number of channels
		mov cl,32
		xor eax,eax
@@getnumlp:     lodsb
		cmp al,0fh
		ja @@nochan
		inc ah
@@nochan:       dec cl
		jnz @@getnumlp
		mov [edi+_TotalChanNum],ah

		mov al,[__SoundDevice]
		mov [edi+_DeviceType],al

		mov ebx,[edi+_Pointer]
		call FindSampsSize
		mov [edi+_SampleSize],eax

@@exit:         clc
		ret
@@abort:        stc
		ret
ParseModule     endp

FindSampsSize   proc near
		xor edx,edx
		push esi edi ebx
		mov esi,ebx

		movzx ecx,word ptr [esi+22h]

		movzx eax,word ptr [esi+20h]
		add eax,ebx
		add eax,60h
		mov edi,eax             ;ESI=Ptr to module, EDI=Ptr to current inst offset

@@countloop:    movzx ebx,word ptr [edi]
		shl ebx,4
		inc edi
		inc edi
		add ebx,esi

		mov eax,[ebx+10h]
		or eax,eax
		jz @@docount
		inc eax
		shr eax,5               ;round to multiple of 32
		inc eax
		shl eax,5
		add edx,eax
@@docount:      dec ecx
		jnz @@countloop

		pop ebx edi esi
		mov eax,edx
		ret
FindSampsSize   endp

%

;----------------------------------------------------------------------------
PM_InitSystem   proc near               ;Init system with device AL
					;Mixing rate BX
					;Buffer size DX (SB only)
		test [__SystemStatus],SYS_INIT
		jz @@initsys
		push eax
		call PM_CloseSystem
		pop eax
@@initsys:      push edx
		mov edx,offset _ret     ;set end handler address to default
		mov [LoopJumpPoint],edx
		mov [LoopCount],0
		pop edx

		; detect/init soundcards here
		cmp al,DEVICE_GUS
		jz @@initgus
		cmp al,DEVICE_SBMONO
		jz @@trysb

		cmp al,DEVICE_AUTO
		jnz @@abort

@@initgus:      push ebx edx
		call GUS_Read_Env       ;detect for gus
		pop edx ebx

		or eax,eax
		jz @@abort
		clc
		jmp @@exit


		or eax,eax
		jz @@trysb
		;call GUS_SetInterface
		jc @@abort
		;call GUS_Init
		jc @@abort
		jmp short @@exit

@@trysb:        push ebx edx
		;call SB_Read_Env        ;detect for sb
		pop edx ebx
		or eax,eax
		jz @@abort
		;call SB_Init
		jc @@abort

@@exit:         mov eax,ChanDataSize*32 ;get memory for channel data
		call _gethimem
		jc @@abort
		mov [__ChannelData],eax
		mov [H_ChannelData],ebx
		clc
		ret
@@abort:        stc

_ret:           ret
PM_InitSystem   endp

;----------------------------------------------------------------------------
PM_CloseSystem  proc near

		test [__SystemStatus],SYS_INIT
		jz @@abort
		test [__SystemStatus],SYS_PLAY
		jz @@nostopit

		call PM_StopModule

@@nostopit:     ;Uninit module/soundcard here
		mov [__SystemStatus],SYS_UNINIT
		cmp [__SoundDevice],DEVICE_GUS
		jnz @@trysb                      ;Pack up gus

		;call GUS_Close
		jmp short @@exit

@@trysb:        cmp [__SoundDevice],DEVICE_SBMONO
		jnz @@abort

		;call SB_Close                   ;Pack up sb

@@exit:         ;sub [_himembase],ChanDataSize*32
		mov eax,[H_ChannelData]
		call _freehimem

		mov [__SoundDevice],DEVICE_UNDEF
		clc
		ret
@@abort:        stc

		ret
PM_CloseSystem  endp

;----------------------------------------------------------------------------
; End music library code
;----------------------------------------------------------------------------

comment %


;----------------------------------------------------------------------------
; Music tracking code
;----------------------------------------------------------------------------
__UpdateTracker proc near               ;Handles tick updates

		mov edx,[__ChannelData]        ;handle VU bars
		movzx ecx,[ebx+_TotalChanNum]
@@handlevuloop: mov al,[edx+_VUBarLevel]
		or al,al
		jz @@nodec
		dec al
		jz @@nodec
		dec al
@@nodec:        mov [edx+_VUBarLevel],al
		add edx,ChanDataSize
		loop @@handlevuloop

		dec [ebx+_MCurrentTick]
		jz @@MajorUpdate
@@MinorUpdate:  ;Place all minor update code here
		;--------------------------------

		mov al,[ebx+_MCurrentTick]
		mov [ebx+_MActualTick],al

		xor cl,cl
		mov esi,[__ChannelData]
@@minorchloop:  push ecx
		call HandleMinor
		push edi
		mov edi,esi
		call ClipPitch
		pop edi
		add esi,ChanDataSize
		pop ecx
		inc cl
		cmp cl,[ebx+_TotalChanNum]
		jb @@minorchloop
		ret
@@MajorUpdate:  ;Place all major update code here
		;--------------------------------

		mov al,[ebx+_MCurrentRow]   ;update values
		mov [ebx+_MActualRow],al
		mov ax,[ebx+_MCurrentPos]
		mov [ebx+_MActualPos],ax
		mov al,[ebx+_MCurrentPatt]
		mov [ebx+_MActualPatt],al

		;First, consider row delay command
		cmp [ebx+_MRowDelay],0
		jz @@norowdelay
		dec [ebx+_MRowDelay]
		jmp @@nonewpattern
@@norowdelay:
		;1. Determine pos + process notes data

		mov edi,[__ChannelData]        ;Blank old commands
		mov cl,[ebx+_TotalChanNum]
@@blankloop:    mov [edi+_CMDVal],0             ;blank info vals
		mov [edi+_CMDData],0

		;mov [edi+_ChannelFlag],0
		mov eax,[edi+_CurrentPeriod]    ;Reset vibrato pitch changes
		cmp eax,[edi+_ActualPeriod]
		je @@noresetpitch
		mov [edi+_ActualPeriod],eax
		or [edi+_ChannelFlag],_CHN_NewPitch
@@noresetpitch:
		cmp [edi+_CommandValue],17      ;Dont reset special value
		je @@noresetcounter             ;  if last command is retrig
		mov [edi+_SpecialValue],0
@@noresetcounter:
		mov [edi+_CommandValue],0
		add edi,ChanDataSize
		dec cl
		jnz @@blankloop

		cmp [ebx+_MRowPointer],0
		jz @@incpoint                   ;abort if not valid pointer

		mov esi,[ebx+_MRowPointer]
		mov edi,[__ChannelData]        ;(set to first chan default)
		;Process current packed track data (ptr at ESI)
@@processloop:  mov ch,[esi]
		inc esi
		cmp ch,0
		je @@endprocess                 ;leave if end of row

		call FetchChannel

		mov [NOTE],255          ;Decompress data
		mov [INSTRUMENT],0
		mov [VOLUME],255
		mov [COMMAND],255
		mov [INFO],0

		test ch,32              ;Test for new note/inst
		jz @@chekforvol
			mov al,[esi]            ;Cant make definate judgement
			inc esi                 ;  until we know if it is
			mov [NOTE],al           ;  a note or porta
			mov al,[esi]
			inc esi
			mov [INSTRUMENT],al     ;  likewise for insts

@@chekforvol:   test ch,64                      ;Check for volume
		jz @@chekforcmd
			mov al,[esi]
			inc esi
			mov [VOLUME],al

@@chekforcmd:   test ch,128                     ;Check for command & data
		jz @@processvalues
			mov al,[esi]
			inc esi
			mov [COMMAND],al
			mov al,[esi]
			inc esi
			mov [INFO],al

@@processvalues: ;Actually interpret decompressed values

		;Check if chan > active channels
		mov al,ch
		and al,00011111b
		cmp al,[ebx+_TotalChanNum]
		jb @@validchan
		jmp @@processloop

@@validchan:    ;Firstly, retrieve new sample info for new inst if any
		movzx eax,[INSTRUMENT]
		call PtrToSample
		jc @@tryanewnote
		mov ecx,eax

		mov al,[INSTRUMENT]
		mov [edi+_SampleNum],al

		movzx eax,word ptr ds:[ecx+20h] ;Now EAX=Sample middle-c value(!)
		or eax,eax
		jz @@notvalidfreq
		mov [edi+_C4SPD],eax
@@notvalidfreq: mov al,ds:[ecx+1ch]             ;And get sample volume
		cmp al,64
		jbe @@novolprobs
		mov al,0
@@novolprobs:   mov [edi+_SampleVolume],al

		;And set values for new sample
		mov [edi+_CurrentVol],al
		mov [edi+_ActualVol],al
		or [edi+_ChannelFlag],_CHN_NewVol

@@tryanewnote:  ;Update note value
		mov al,[NOTE]
		cmp al,255              ;Check for no new note
		je @@nonewnote
		cmp al,254              ;Check for sample cut
		jne @@yesnewnote
@@cutnote:      mov [edi+_CurrentNote],0        ;Cut old sample here
		mov [edi+_TargetNote],0
		mov [edi+_CurrentPeriod],1712
		mov [edi+_TargetPeriod],1712
		mov [edi+_ActualPeriod],1712
		mov [edi+_SampleNum],255
		mov [edi+_SampleOffset],0
		mov [edi+_VUBarLevel],0
		or [edi+_ChannelFlag],_CHN_NewSamp
		jmp @@nonewnote

@@yesnewnote:   ;***Check for porta here***!!

		cmp [edi+_ActiveFlag],0         ;new note if no note playing
		jz @@normalnote

		cmp [COMMAND],7
		je @@setforporta
		cmp [COMMAND],12
		jne @@normalnote
@@setforporta:  cmp [edi+_SampleNum],255          ;only if sample playing
		jz @@normalnote
		mov [edi+_TargetNote],al        ;set target note for info
		call PeriodFromNote             ;Set for portamento
		mov [edi+_TargetPeriod],eax
		jmp short @@valuesset
@@normalnote:   mov [edi+_CurrentNote],al       ;Set for normal note
		mov [edi+_TargetNote],al
		mov [edi+_SampleOffset],0
		call PeriodFromNote
		mov [edi+_CurrentPeriod],eax
		mov [edi+_TargetPeriod],eax
		mov [edi+_ActualPeriod],eax
		mov al,[edi+_CurrentVol]
		mov [edi+_VUBarLevel],al
		or [edi+_ChannelFlag],_CHN_NewSamp
@@valuesset:    or [edi+_ChannelFlag],_CHN_NewPitch

@@nonewnote:    cmp [VOLUME],255
		je @@handledata
		mov al,[VOLUME]
		mov [edi+_CurrentVol],al
		mov [edi+_ActualVol],al
		mov [edi+_VUBarLevel],al
		or [edi+_ChannelFlag],_CHN_NewVol

@@handledata:   call __HandleCommands           ;Time to process command and data bytes
		call ClipPitch
@@noamigalimit: jmp @@processloop

@@endprocess:   mov [ebx+_MRowPointer],esi
@@incpoint:     ;2. increment row/pos counter
		inc [ebx+_MCurrentRow]
		cmp [ebx+_MCurrentRow],64
		jb @@nonewpattern               ;Determine next pattern
		mov [ebx+_MRowLoopStart],0      ;Reset patloop posn to start of pattern
@@inccounter:   inc [ebx+_MCurrentPos]          ;Check if past end of orderlist
		mov ax,[ebx+_MCurrentPos]
		cmp ax,[ebx+_Ordnum]
		jb @@calcnextpatt
		mov [ebx+_MCurrentPos],0        ;<==Add reset tempo/speed here also

		cmp ax,254                      ;inc song loop counter
		jae @@calcnextpatt
		inc [LoopCount]

@@calcnextpatt: mov esi,[ebx+_Pointer]          ;<==Add FX jump past start (Jxx)
		movzx eax,word ptr [ebx+_MCurrentPos] ;get ptr to orderlist+current pos
		add eax,060h
		add esi,eax
		movzx eax,byte ptr [esi]
		cmp al,254
		jae @@inccounter
		cmp ax,[ebx+_Patnum]            ;check not an invalid patt
		jae @@inccounter
		mov [ebx+_MCurrentPatt],al      ;set current track

		movzx eax,byte ptr [ebx+_MCurrentPatt] ;Calc offset of pattern in mem
		add eax,eax
		add ax,[ebx+_Insnum]    ;\
		add ax,[ebx+_Insnum]    ;|__ These should not pass 64k
		add ax,[ebx+_Ordnum]    ;|
		add ax,60h              ;/
		add eax,[ebx+_Pointer]
		mov esi,eax             ;Calced ptr to pattn ptr
		movzx eax,word ptr [esi]
		shl eax,4
		inc eax
		inc eax
		add eax,[ebx+_Pointer]
		mov cl,[ebx+_BreakToRow]
		mov [ebx+_MCurrentRow],cl
		or cl,cl
		jz @@notbreak2row
		call FindPatternPos
@@notbreak2row: mov [ebx+_MRowPointer],eax      ;(reset row pointer)
@@nonewpattern: mov al,[ebx+_MCurrentSpd]       ;3. Update tick counter
		mov [ebx+_MCurrentTick],al
		ret
__UpdateTracker endp

ClipPitch       proc near               ;clip to amiga limits if necessary
		test [edi+_ChannelFlag],_CHN_NewPitch
		jz @@above
		test [edi+_GlissFlag],1
		jz @@checkami

		push ebx esi            ;handle glissando on pitch
		mov eax,[edi+_ActualPeriod]
		mul dword ptr [edi+_C4SPD]
		mov ebx,8363*16
		idiv ebx
		mov ebx,eax             ;scan for value
		xor ecx,ecx
		xor edx,edx
@okeygo:        mov esi,offset Period_Table
		mov cl,12
@nextpitchv:    mov eax,ebx             ;loop and sub each one until pos
		sub ax,[esi]
		jge @aboutit
		mov dx,[esi]
		add esi,2
		dec cl
		jnz @nextpitchv
		shl ebx,1               ;double and try again
		inc ch
		jmp @okeygo
@aboutit:       sub dx,bx               ;find nearest
		cmp dx,ax
		jae @riteone
		sub esi,2
@riteone:       mov cl,ch
		movzx eax,word ptr [esi] ;ax=nearest pitch
		imul eax,eax,8363*16
		shr eax,cl
		cdq
		idiv dword ptr [edi+_C4SPD]
		or eax,eax
		jnz @@snotzero
		inc eax
@@snotzero:     mov [edi+_ActualPeriod],eax
		pop esi ebx

@@checkami:     test [ebx+_generalflags],16     ;clip pitch to limits if required
		jz @@above
		mov eax,[edi+_ActualPeriod]
		cmp eax,856*4
		jbe @@below
		mov [edi+_ActualPeriod],856*4
		or [edi+_ChannelFlag],_CHN_NewPitch
@@below:        cmp eax,113*4
		jae @@above
		mov [edi+_ActualPeriod],113*4
		or [edi+_ChannelFlag],_CHN_NewPitch
@@above:        ret
ClipPitch       endp

FindPatternPos  proc near               ;Seek to row CL
@@checktoobig:  cmp cl,63
		jbe @@doseek             ;if seek>63, set for new pattern
		mov [ebx+_BreakToRow],0
		xor eax,eax
		ret
@@doseek:       mov ch,ds:[eax]
		inc eax
		or ch,ch
		jz @@deccounter2
		test ch,32              ;Test for new note/inst
		jz @@chekforvol2
		inc eax
		inc eax
@@chekforvol2:  test ch,64              ;Test for volume
		jz @@chekforcmd2
		inc eax
@@chekforcmd2:  test ch,128             ;Test for command & data
		jz @@doseek
		inc eax
		inc eax
		jmp @@doseek
@@deccounter2:  dec cl
		jnz @@doseek
		mov [ebx+_BreakToRow],0
		ret
FindPatternPos  endp

;----------------------------------------------------------------------------
; Static FX handlers
;----------------------------------------------------------------------------

StaticJumpTable dd      offset S_FX_0
		dd      offset S_FX_A   ;A
		dd      offset S_FX_B
		dd      offset S_FX_C
		dd      offset S_FX_D
		dd      offset S_FX_E
		dd      offset S_FX_F
		dd      offset S_FX_G
		dd      offset S_FX_H
		dd      offset S_FX_I
		dd      offset S_FX_J
		dd      offset S_FX_K
		dd      offset S_FX_L
		dd      offset S_FX_0
		dd      offset S_FX_0
		dd      offset S_FX_O
		dd      offset S_FX_0
		dd      offset S_FX_Q
		dd      offset S_FX_H   ;tremolo uses same pars as vibrato
		dd      offset S_FX_S
		dd      offset S_FX_T
		dd      offset S_FX_H   ;fine vibrato uses same pars as vibrato
		dd      offset S_FX_V
		dd      offset S_FX_0
		dd      offset S_FX_X
		dd      offset S_FX_0
		dd      offset S_FX_0   ;Z

;----------------------------------------------------------------------------

__HandleCommands proc near              ;Process current command
		mov al,[INFO]
		mov [edi+_DataValue],al
		mov [edi+_CMDData],al
		movzx eax,[COMMAND]
		mov [edi+_CommandValue],al
		mov [edi+_CMDVal],al
		cmp al,255              ;Check for no command
		je S_FX_0
		cmp al,26
		ja @endmajor
		call [StaticJumpTable+eax*4]    ;Jump to command
@endmajor:      ret
__HandleCommands endp

;----------------------------------
; Command handle code
;----------------------------------
S_FX_0          proc near               ;No command
		mov [edi+_DataValue],0
		mov [edi+_CommandValue],0
		ret
S_FX_0          endp

S_FX_A          proc near               ;Axx = set song speed
		mov al,[INFO]
		mov [ebx+_MCurrentSpd],al
		jmp S_FX_0
S_FX_A          endp

S_FX_B          proc near               ;Bxx = jump to order
		xor eax,eax
		mov al,[INFO]
		dec eax
		mov [ebx+_MCurrentPos],ax
		mov [ebx+_MCurrentRow],0feh
		mov [ebx+_BreakToRow],0
		jmp S_FX_0
S_FX_B          endp

S_FX_C          proc near               ;Cxx = break current pattern
		mov [ebx+_MCurrentRow],0feh
		mov cl,[INFO]
		mov al,cl               ;convert decimal to hex
		shr al,4
		mov dl,10
		mul dl
		and cl,0fh
		add cl,al
		mov [ebx+_BreakToRow],cl
		jmp S_FX_0
S_FX_C          endp

S_FX_D          proc near               ;Dxx = volume slide

		mov al,[INFO]
		or al,al
		jnz @@newvolspeed
		mov al,[edi+_VolSlideValue]
@@newvolspeed:  mov ah,al
		cmp al,0F0h
		jbe @@notspotvoldown            ;Instant slide down
		mov al,ah
		and al,00Fh
		mov dl,[edi+_ActualVol]
		xchg dl,al
		sub al,dl
		cmp al,64
		jna @@validspotvolume
		mov al,0
		jmp short @@validspotvolume

@@notspotvoldown: mov al,ah
		and al,00Fh
		cmp al,00Fh
		jnz @@notspotvolup              ;Instant slide up
		mov al,ah
		shr al,4
		or al,al
		jz @@notspotvolup
		add al,[edi+_ActualVol]
		cmp al,64
		jb @@validspotvolume
		mov al,64
@@validspotvolume: mov [edi+_CurrentVol],al     ;Set values and quit
		mov [edi+_ActualVol],al
		mov [edi+_VolSlideValue],ah
		or [edi+_ChannelFlag],_CHN_NewVol
		jmp S_FX_0

@@notspotvolup: mov al,ah                       ;Set for normal volume slide
		mov [edi+_DataValue],al
		mov [edi+_VolSlideValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
S_FX_D          endp

S_FX_E          proc near               ;Exx = slide pitch down
		mov al,[INFO]
		or al,al                ;if zero get old slide value
		jnz @@validslide
		mov al,[edi+_VolSlideValue]
@@validslide:   mov [edi+_VolSlideValue],al     ;Normal slide down
		cmp al,0dfh
		ja @@instantslide
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
@@instantslide: cmp al,0efh
		ja @@fineslide
		and eax,0000000Fh               ;Extra fine slide (x1)
		add [edi+_CurrentPeriod],eax
		mov eax,[edi+_CurrentPeriod]
		mov [edi+_ActualPeriod],eax
		or [edi+_ChannelFlag],_CHN_NewPitch
		jmp S_FX_0
@@fineslide:    and eax,0000000Fh               ;Fine slide (x4)
		shl eax,2
		add [edi+_CurrentPeriod],eax
		mov eax,[edi+_CurrentPeriod]
		mov [edi+_ActualPeriod],eax
		or [edi+_ChannelFlag],_CHN_NewPitch
		jmp S_FX_0
S_FX_E          endp

S_FX_F          proc near               ;Fxx = slide pitch up
		mov al,[INFO]
		or al,al                ;if zero get old slide value
		jnz @@validslide
		mov al,[edi+_VolSlideValue]
@@validslide:   mov [edi+_VolSlideValue],al     ;Normal slide up
		cmp al,0dfh
		ja @@instantslide
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
@@instantslide: cmp al,0efh
		ja @@fineslide
		and eax,0000000Fh               ;Extra fine slide (x1)
		sub [edi+_CurrentPeriod],eax
		mov eax,[edi+_CurrentPeriod]
		mov [edi+_ActualPeriod],eax
		or [edi+_ChannelFlag],_CHN_NewPitch
		jmp S_FX_0
@@fineslide:    and eax,0000000Fh               ;Fine slide (x4)
		shl eax,2
		sub [edi+_CurrentPeriod],eax
		mov eax,[edi+_CurrentPeriod]
		mov [edi+_ActualPeriod],eax
		or [edi+_ChannelFlag],_CHN_NewPitch
		jmp S_FX_0
S_FX_F          endp

S_FX_G          proc near               ;Gxx = Portamento to note
		mov al,[INFO]
		or al,al
		jnz @@validvalue
		mov al,[edi+_PortaValue]
@@validvalue:   mov [edi+_PortaValue],al
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
S_FX_G          endp

S_FX_H          proc near               ;Hxx = Vibrato
		mov al,[INFO]
		or al,al
		jnz @@checkvalue
		mov al,[edi+_VibValue]
		jmp short @@foundvalue
@@checkvalue:   cmp al,0fh
		ja @@foundvalue
		mov ah,[edi+_VibValue]
		and ah,0f0h
		or al,ah
@@foundvalue:   mov [edi+_VibValue],al
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		test [edi+_ChannelFlag],_CHN_NewSamp
		jz @noresetcount
		mov [edi+_VibCount],0
@noresetcount:  ret
S_FX_H          endp

S_FX_I          proc near               ;Ixx = Tremor note
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		mov al,[INFO]
		or al,al
		jnz @@newtremor
		mov al,[edi+_DataValue]
@@newtremor:    mov [edi+_DataValue],al
		push esi
		mov esi,edi
		call M_FX_I
		pop esi
		ret
S_FX_I          endp

S_FX_J          proc near               ;Jxx = Arpeggio
		mov [edi+_ArpCount],1
		mov al,[INFO]
		or al,al
		jnz @@newarpvalue
		mov al,[edi+_ArpValue]
@@newarpvalue:  mov [edi+_ArpValue],al
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		push esi
		mov esi,edi
		call M_FX_J
		pop esi
		ret
S_FX_J          endp

S_FX_K          proc near               ;Kxx = Vibrato + Vol. Slide (H00 & Dxx)
		push eax
		mov al,[INFO]
		push eax
		call S_FX_D
		mov [INFO],0
		call S_FX_H
		pop eax
		mov [edi+_DataValue],al
		pop eax
		mov [edi+_CommandValue],al
		ret
S_FX_K          endp

S_FX_L          proc near               ;Lxx = Porta + Vol. Slide (G00 & Dxx)
		push eax
		mov al,[INFO]
		push eax
		call S_FX_D
		mov [INFO],0
		call S_FX_G
		pop eax
		mov [edi+_DataValue],al
		pop eax
		mov [edi+_CommandValue],al
		ret
S_FX_L          endp

S_FX_O          proc near               ;Oxx = sample offset
		xor eax,eax             ;<== account for offset > sample size
		mov ah,[INFO]
		or ah,ah
		jnz @@gotoffset
		mov ah,[edi+_OffsetValue]
@@gotoffset:    mov [edi+_OffsetValue],ah
		mov [edi+_SampleOffset],ax
		cmp [edi+_CurrentNote],0        ;make sure note is present
		jz @@abort
		or [edi+_ChannelFlag],_CHN_NewSamp
@@abort:        jmp S_FX_0
S_FX_O          endp

S_FX_Q          proc near               ;Qxx = retrig note
		mov al,[INFO]
		cmp al,0
		jnz @@newretrigval
		mov al,[edi+_RetrigValue]
@@newretrigval: mov [edi+_RetrigValue],al
		and al,0fh

		cmp [edi+_SpecialValue],0       ;consider past retrigs
		je @@setcount

		push esi                        ;handle if past retrig active
		mov esi,edi
		call M_FX_Q
		pop esi
		jmp @@nosetcount

@@setcount:     mov [edi+_SpecialValue],al
@@nosetcount:   mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
S_FX_Q          endp

S_FX_S          proc near               ;Sxx = Special comands
		mov al,[INFO]
		mov ah,al
		and ah,0fh
		shr al,4

		cmp al,1                ;S1x = set glissando flag
		jnz @@check2
		mov al,[INFO]
		and al,0fh
		or al,al
		jz @setgflag
		mov al,1
@setgflag:      mov [edi+_GlissFlag],al
		jmp S_FX_0

@@check2:       cmp al,2                ;S2x = set finetune
		jnz @@check3
		movzx eax,[INFO]
		and al,0fh
		mov ax,[FineTuneTable+eax*2]
		mov [edi+_C4SPD],eax
		jmp S_FX_0
@@check3:       cmp al,3
		jnz @@check4            ;S3x = set vibrato waveform
		cmp ah,3
		jb @noajustv
		sub ah,4
		mov [edi+_VibCount],0   ;reset vibcount if > 3
@noajustv:      and ah,3
		mov [edi+_VibTable],ah
		jmp S_FX_0
@@check4:       cmp al,4                ;S4x = set tremolo waveform
		jnz @@check8
		cmp ah,3
		jb @noajustt
		sub ah,4
		mov [edi+_VibCount],0
@noajustt:      and ah,3
		mov [edi+_TremTable],ah
		jmp S_FX_0
@@check8:       cmp al,8                ;S8x = set channel balance
		jnz @@checkB
		mov [edi+_PanPosition],ah
		or [edi+_ChannelFlag],_CHN_NewPan
		jmp S_FX_0
@@checkB:       cmp al,0Bh              ;SBx = inner pattern loop
		jnz @@checkC
		or ah,ah
		jz @@setlcounter
		;SBx - set loop count
		mov al,[ebx+_MRowLoopCount]
		cmp al,ah
		jb @@justjump
		mov [ebx+_MRowLoopCount],0
		jmp S_FX_0
@@justjump:     inc [ebx+_MRowLoopCount]
		dec [ebx+_MCurrentPos]
		mov [ebx+_MCurrentRow],0feh
		mov al,[ebx+_MRowLoopStart]
		mov [ebx+_BreakToRow],al
		jmp S_FX_0
@@setlcounter:  ;SB0 - Set loop start
		mov al,[ebx+_MCurrentRow]
		mov [ebx+_MRowLoopStart],al
		jmp S_FX_0
@@checkC:       cmp al,0Ch              ;SCx = Note cut
		jnz @@checkD
		mov [edi+_SpecialValue],ah
		mov al,[INFO]
		mov [edi+_DataValue],al
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
@@checkD:       cmp al,0Dh              ;SDx = Note delay
		jnz @@checkE
		mov al,[INFO]
		mov [edi+_DataValue],al
		mov al,[edi+_ChannelFlag]
		mov [edi+_SpecialValue],al
		mov [edi+_ChannelFlag],0
		mov al,[COMMAND]
		mov [edi+_CommandValue],al
		ret
@@checkE:       cmp al,0eh              ;SEx = Row delay by x notes
		jnz @@checkF
		mov al,[INFO]
		and al,0fh
		mov [ebx+_MRowDelay],al
@@checkF:       jmp S_FX_0              ;SFx = Not supported by ST3 (funkrepeat)
S_FX_S          endp

S_FX_T          proc near               ;Txx = Set tempo (BPM)
		mov al,[INFO]
		cmp al,020h
		jae @@nofixbpm
		mov al,020h             ;BPM must be > 20h
@@nofixbpm:     mov [ebx+_MCurrentBPM],al
		or [edi+_ChannelFlag],_CHN_NewBPM
		jmp S_FX_0
S_FX_T          endp

S_FX_V          proc near               ;Vxx = set global volume
		mov al,[INFO]
		cmp al,64
		jng @@checklow
		mov al,64
		jmp short @@setglobal
@@checklow:     cmp al,0
		jge @@setglobal
		mov al,0
@@setglobal:    mov [ebx+_RealGlobalVol],al
		call SetAllVol
		jmp S_FX_0
S_FX_V          endp

S_FX_X          proc near               ;Xxx = set pan position
		mov al,[INFO]
		shr al,3
		cmp al,010h
		jb @@noadjust
		dec al
@@noadjust:     and al,0fh
		mov [edi+_PanPosition],al
		or [edi+_ChannelFlag],_CHN_NewPan
		jmp S_FX_0
S_FX_X          endp

;----------------------------------------------------------------------------
MinorJumpTable  dd      offset M_FX_0
		dd      offset M_FX_0   ;A
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_D
		dd      offset M_FX_E
		dd      offset M_FX_F
		dd      offset M_FX_G
		dd      offset M_FX_H
		dd      offset M_FX_I
		dd      offset M_FX_J
		dd      offset M_FX_K
		dd      offset M_FX_L
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_Q
		dd      offset M_FX_R
		dd      offset M_FX_S
		dd      offset M_FX_0
		dd      offset M_FX_U
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_0
		dd      offset M_FX_0   ;Z

;----------------------------------------------------------------------------
HandleMinor     proc near               ;Handle minor fx for channel ptr ESI
		movzx eax,byte ptr [esi+_CommandValue]
		or al,al
		jz @@endminor
		cmp al,26
		jnb @@endminor
		call [MinorJumpTable+eax*4]    ;Jump to command
@@endminor:     ret
HandleMinor     endp

;----------------------------------------------------------------------------
; Progressive FX code:
;----------------------------------------------------------------------------
M_FX_0          proc near               ;No minor fx
		ret
M_FX_0          endp

M_FX_D          proc near               ;Dxx = Volume slide
		mov al,[esi+_VolSlideValue]
		and al,0F0h
		jz @@slidevoldown
		shr al,4                        ;add volume (slide up)
		add al,[esi+_ActualVol]
		cmp al,64
		jbe @@withinlimits
		mov al,64
		jmp short @@withinlimits
@@slidevoldown: mov al,[esi+_ActualVol]        ;else subtract volume
		mov ah,[esi+_VolSlideValue]
		and ah,00fh
		sub al,ah
		cmp al,64
		jna @@withinlimits              ;ensure vol >= 0
		mov al,0
@@withinlimits: mov [esi+_CurrentVol],al
		mov [esi+_ActualVol],al
		or [esi+_ChannelFlag],_CHN_NewVol
		ret
M_FX_D          endp

M_FX_E          proc near               ;Exx = Pitch slide down by xx
		movzx eax,byte ptr [esi+_DataValue]
		shl eax,2
		add [esi+_CurrentPeriod],eax
		mov eax,[esi+_CurrentPeriod]
		mov [esi+_ActualPeriod],eax
		or [esi+_ChannelFlag],_CHN_NewPitch
		ret
M_FX_E          endp

M_FX_F          proc near               ;Fxx = Pitch slide up by xx
		movzx eax,byte ptr [esi+_DataValue]
		shl eax,2
		sub [esi+_CurrentPeriod],eax
		mov eax,[esi+_CurrentPeriod]
		mov [esi+_ActualPeriod],eax
		or [esi+_ChannelFlag],_CHN_NewPitch
		ret
M_FX_F          endp

M_FX_G          proc near               ;Gxx = portamento to note
		push ecx
		mov eax,[esi+_CurrentPeriod]
		cmp eax,[esi+_TargetPeriod]
		jz @@finishedporta
		ja @@slideup
@@slidedown:    movzx edx,byte ptr [esi+_PortaValue] ;Slide down (inc period value)
		shl edx,2
		mov ecx,[esi+_TargetPeriod]
		sub ecx,eax
		cmp ecx,edx
		jbe @@setdownsld        ;prevent slide overflows
		add eax,edx
		cmp eax,[esi+_TargetPeriod]
		jb @@finishedporta
@@setdownsld:   mov eax,[esi+_TargetPeriod]
		jmp short @@finishedporta
@@slideup:      movzx edx,byte ptr [esi+_PortaValue] ;Slide up (dec period value)
		shl edx,2
		mov ecx,eax
		sub ecx,[esi+_TargetPeriod]
		cmp ecx,edx
		jbe @@setupsld
		sub eax,edx
		cmp eax,[esi+_TargetPeriod]
		ja @@finishedporta
@@setupsld:     mov eax,[esi+_TargetPeriod]
@@finishedporta: mov [esi+_CurrentPeriod],eax
		mov [esi+_ActualPeriod],eax
		or [esi+_ChannelFlag],_CHN_NewPitch
		pop ecx
		ret
M_FX_G          endp

M_FX_H          proc near               ;Hxx = Note Vibrato
		xor eax,eax             ;First, change current pitch
		xor edx,edx
		mov al,[esi+_VibTable]  ;select vibrato table
		mov eax,[Vibrato_Tables+eax*4]
		mov dl,[esi+_VibCount]  ;get offset in table
		movsx edx,word ptr [eax+edx*2] ;and get period value
		sal edx,2

		movzx eax,byte ptr [esi+_VibValue] ;multiply for amplification
		and al,00fh
		imul edx
		sar eax,7

		add eax,[esi+_CurrentPeriod]
		mov [esi+_ActualPeriod],eax ;store modified pitch
		or [esi+_ChannelFlag],_CHN_NewPitch
		mov al,[esi+_VibValue] ;Then, increase table counter
		shr al,4
		add al,[esi+_VibCount]
		cmp al,64
		jb @@setviboffset
		sub al,64
@@setviboffset: mov [esi+_VibCount],al
		ret
M_FX_H          endp

M_FX_I          proc near               ;Ixx = Tremor volume
		cmp [esi+_TremorCount],0
		jz @@dotremor
		dec [esi+_TremorCount]
		ret
@@dotremor:     mov al,[esi+_DataValue]
		cmp [esi+_TremorFlag],1         ;Restore volume
		je @@makevoloff
		mov [esi+_TremorFlag],1
		shr al,4
		mov [esi+_TremorCount],al
		mov al,[edi+_CurrentVol]
		mov [esi+_ActualVol],al
		or [esi+_ChannelFlag],_CHN_NewVol
		ret
@@makevoloff:   mov [esi+_TremorFlag],0         ;Zero volume
		and al,0fh
		mov [esi+_TremorCount],al
		mov [esi+_ActualVol],0
		or [esi+_ChannelFlag],_CHN_NewVol
		ret
M_FX_I          endp

M_FX_J          proc near               ;Jxx = Arpeggio note
		push edi
		mov edi,esi
		mov dl,[esi+_CurrentNote]
		mov dh,dl
		and dx,0f00fh           ;DH=Current oct, DL=Current note
		dec [esi+_ArpCount]
		jz @@dobasenote
		cmp [esi+_ArpCount],2
		jz @@do1ststep
		mov al,[esi+_ArpValue] ;Set note for 2nd arpeggio step
		and al,0fh
		add dl,al
		jmp short @@setpitch
@@do1ststep:    mov al,[esi+_ArpValue]
		shr al,4
		add dl,al
		jmp short @@setpitch
@@dobasenote:   mov [esi+_ArpCount],3
@@setpitch:     cmp dl,12                       ;adjust for octaves
		jb @@nooctinc
		add dh,010h
		sub dl,12
@@nooctinc:     or dh,dl
		mov [NOTE],dh
		call PeriodFromNote
		mov [esi+_ActualPeriod],eax
		or [esi+_ChannelFlag],_CHN_NewPitch
		pop edi
		ret
M_FX_J          endp

M_FX_K          proc near               ;Kxx = Dual H00 & Dxx
		call M_FX_D
		call M_FX_H
		ret
M_FX_K          endp

M_FX_L          proc near               ;Kxx = Dual G00 & Dxx
		call M_FX_D
		call M_FX_G
		ret
M_FX_L          endp

M_FX_Q          proc near               ;Qxx = Retrigger note
		dec [esi+_SpecialValue]
		jz @@doretrig
@noretrig:      ret
@@doretrig:     cmp [esi+_CurrentNote],0        ;check if note is present
		jz @noretrig

		push ebx
		mov al,[esi+_RetrigValue] ;start new note
		and al,0fh
		mov [esi+_SpecialValue],al
		mov [esi+_SampleOffset],0
		or [esi+_ChannelFlag],_CHN_NewSamp
		mov al,[esi+_RetrigValue] ;perform volume changes
		shr al,4
		cmp al,06h                ;handle multiply volumes
		jz @@times23
		cmp al,07h
		jz @@times12
		cmp al,0eh
		jz @@times32
		cmp al,0fh
		jz @@times21
		mov ebx,offset Retrig_Table
		xlat
		add al,[esi+_ActualVol]
		cbw
		jmp short @@doneretrig
@@times23:      movzx eax,byte ptr [esi+_ActualVol]
		add al,al
		mov bl,3
		div bl
		xor ah,ah
		jmp short @@doneretrig
@@times12:      mov al,[esi+_ActualVol]
		shr al,1
		jmp short @@doneretrig
@@times32:      movzx eax,byte ptr [esi+_ActualVol]
		mov bl,3
		mul bl
		shr eax,1
		jmp short @@doneretrig
@@times21:      mov al,[esi+_ActualVol]
		cbw
		add eax,eax
@@doneretrig:   cmp al,64               ;check volume extents
		jle @@checklow
		mov al,64
@@checklow:     cmp al,0
		jge @@validvol
		xor eax,eax
@@validvol:     mov [esi+_CurrentVol],al
		mov [esi+_ActualVol],al
		or [esi+_ChannelFlag],_CHN_NewVol
		pop ebx
		ret
M_FX_Q          endp

M_FX_R          proc near               ;Rxx = tremolo
		xor eax,eax             ;First, change current volume
		xor edx,edx
		mov al,[esi+_TremTable]  ;select tremolo table
		mov eax,[Vibrato_Tables+eax*4]
		mov dl,[esi+_VibCount]  ;get offset in table
		movsx edx,word ptr [eax+edx*2] ;and get period value
		;sal edx,2
		;movsx edx,dx
		;sar edx,2

		movzx eax,byte ptr [esi+_VibValue] ;multiply for amplification
		and al,00fh
		imul edx
		sar eax,7

		movzx edx,byte ptr [esi+_CurrentVol]
		add eax,edx
@@doneretrig:   cmp eax,64               ;check volume extents
		jle @@checklow
		mov eax,64
@@checklow:     or eax,eax
		jge @@validvol
		xor eax,eax
@@validvol:     mov [esi+_ActualVol],al
		or [esi+_ChannelFlag],_CHN_NewVol

		mov al,[esi+_VibValue] ;Then, increase table counter
		shr al,4
		add al,[esi+_VibCount]
		cmp al,64
		jbe @@setviboffset
		sub al,64
@@setviboffset: mov [esi+_VibCount],al
		ret
M_FX_R          endp

M_FX_S          proc near               ;Sxx = Special FX
		mov al,[esi+_DataValue]
		shr al,4

@@checkC:       cmp al,0Ch                      ;SCx = cut note
		jnz @@checkD
		dec [esi+_SpecialValue]
		jnz @@doneSFX
		mov [esi+_CurrentNote],0        ;Cut old sample here
		mov [esi+_TargetNote],0
		mov [esi+_CurrentPeriod],1712
		mov [esi+_TargetPeriod],1712
		mov [esi+_ActualPeriod],1712
		mov [esi+_SampleNum],255
		mov [esi+_SampleOffset],0
		or [esi+_ChannelFlag],_CHN_NewSamp
		mov [esi+_CommandValue],0
		ret
@@checkD:       cmp al,0Dh              ;SDx = note delay
		jnz @@checkE
		mov al,[esi+_DataValue]
		and al,0fh
		dec al
		jz @@yesD
		or al,0D0h
		mov [esi+_DataValue],al
		jmp short @@doneSFX
@@yesD:         mov al,[esi+_SpecialValue]
		mov [esi+_ChannelFlag],al
		mov [esi+_CommandValue],0
		ret
@@checkE:
@@doneSFX:      ret
M_FX_S          endp

M_FX_U          proc near               ;Uxx = fine vibrato
		xor eax,eax             ;First, change current pitch
		xor edx,edx
		mov al,[esi+_VibTable]  ;select vibrato table
		mov eax,[Vibrato_Tables+eax*4]
		mov dl,[esi+_VibCount]  ;get offset in table
		movsx edx,word ptr [eax+edx*2] ;and get period value

		movzx eax,byte ptr [esi+_VibValue] ;multiply for amplification
		and al,00fh
		imul edx
		sar eax,7

		add eax,[esi+_CurrentPeriod]
		mov [esi+_ActualPeriod],eax ;store modified pitch
		or [esi+_ChannelFlag],_CHN_NewPitch
		mov al,[esi+_VibValue] ;Then, increase table counter
		shr al,4
		add al,[esi+_VibCount]
		cmp al,64
		jb @@fsetviboffset
		sub al,64
@@fsetviboffset: mov [esi+_VibCount],al
		ret
M_FX_U          endp

;----------------------------------------------------------------------------
PeriodFromNote  proc near               ;Return a period value from note
		push ebx
		movzx ebx,[NOTE]
		mov cl,bl
		shr cl,4
		and bl,000fh
		add ebx,ebx
		movzx eax,[Period_Table+bx]
		imul eax,eax,8363*16
		shr eax,cl
		cdq
		idiv dword ptr [edi+_C4SPD]
		or eax,eax
		jnz @@notzeroerror
		inc eax
@@notzeroerror: pop ebx
		ret
PeriodFromNote  endp

;----------------------------------------------------------------------------
PeriodToPitch   proc                    ;Convert period in EAX to frequency
		push edx
		push ebx
		mov ebx,eax             ;note_herz=14317056 / note_st3period
		mov eax,0da7600h
		xor edx,edx
		div ebx
		pop ebx
		pop edx
		ret
PeriodToPitch   endp

;----------------------------------------------------------------------------
FetchChannel    proc near               ;Returns ptr to channel data in EDI
		movzx eax,ch            ;Read packed char in CH
		and al,31
		mov edx,ChanDataSize
		mul edx
		add eax,[__ChannelData]
		mov edi,eax
		ret
FetchChannel    endp

;----------------------------------------------------------------------------
LoadPanSettings proc near       ;load default pan settings (if present)
				;ST version 3.2 and above
		mov esi,[ebx+_Pointer]
		cmp byte ptr [esi+035h],252     ;check if settings present
		jz @setpanposns
		ret
@setpanposns:   push ebx                        ;determine offset of settings
		mov eax,60h
		add ax,[ebx+_Ordnum]
		add ax,[ebx+_Insnum]
		add ax,[ebx+_Insnum]
		add ax,[ebx+_Patnum]
		add ax,[ebx+_Patnum]
		add esi,eax

		mov ebx,[__ChannelData]
		mov cl,32
@@setploop:     mov al,[ebx+_PanPosition]
		test byte ptr [esi],00100000b
		jz @nochangepan
		mov al,[esi]
		and al,00001111b
@nochangepan:   mov [ebx+_PanPosition],al
		inc esi
		add ebx,ChanDataSize
		dec cl
		jnz @@setploop
		pop ebx
		ret
LoadPanSettings endp

;----------------------------------------------------------------------------
ClearChannels   proc near                       ;Clear player channels
		mov dl,[ebx+_stereoflag]
		mov esi,[ebx+_Pointer]
		push ebx
		mov ebx,[__ChannelData]
		xor ecx,ecx
@@clearloop:    call blankchannel
		add ebx,ChanDataSize
		inc cl
		cmp cl,32
		jb @@clearloop
		pop ebx
		ret
blankchannel:   ;Clear channel at ebx
		xor eax,eax             ;clear all quickly
		push edi ecx
		mov edi,ebx
		mov ecx,ChanDataSize
		rep stosb
		pop ecx edi
		mov [ebx+_ChannelNumber],cl     ;set values other than zero
		mov [ebx+_SampleNum],255
		mov [ebx+_CurrentPeriod],1712
		mov [ebx+_TargetPeriod],1712
		mov [ebx+_ActualPeriod],1712
		mov [ebx+_ChannelFlag],_CHN_NewPan ;<== set default pan positions
		mov [ebx+_C4SPD],8363

		mov al,7
		cmp dl,1
		jnz @@setbalance
		push edx                ;Determine channel pan
		mov al,[esi+ecx+40h]
		cmp al,128
		jb @@validchan          ;if not valid chan then set to centre
		mov al,7
		jmp short @@setbalancep
@@validchan:    cmp al,8
		jb @@leftchan
		mov al,0ch              ;<==Default right pan position
		jmp short @@setbalancep
@@leftchan:     mov al,03h              ;<==Default left pan position
@@setbalancep:  pop edx
@@setbalance:   mov [ebx+_PanPosition],al       ;<== set default pan positions
		ret
ClearChannels   endp

;----------------------------------------------------------------------------
PtrToSample     proc near               ;Return pointer to sample number AL in EAX
		or al,al
		jz @@notvalidsample

		movzx eax,al
		dec eax
		add eax,eax
		add ax,[ebx+_Ordnum]
		add eax,[ebx+_Pointer]

		movzx eax,word ptr ds:[eax+60h]

		shl eax,4
		add eax,[ebx+_Pointer]

		cmp byte ptr ds:[eax],1          ;Make sure is a valid sample
		jne @@notvalidsample

		clc
		ret
@@notvalidsample: stc
		ret
PtrToSample     endp

;----------------------------------------------------------------------------
; DMAC General routines
;----------------------------------------------------------------------------
		; This routine programs the DMAC for channels 0-7
		; IN: [DMAChannel], [DMAbaseAdd], [DMApageReg] must be setup
		;     [DMABaseAdd] =  Memory Address port
		;     dh = mode
		;     ax = address
		;     cx = length
		;     dl = page
ProgramDMAChip  proc near
		push ebx
		mov ebx,eax

		cmp [DMAChannel],4
		jb @@DoDMA03

		shr cx,1

		mov al,[DMAChannel]
		out 0D4h,al         ; mask reg bit

		sub al,al
		out 0D8h,al         ; clr byte ptr

		mov al,[DMAChannel]
		sub al,4
		add al,dh
		out 0D6h,al         ; set mode reg

		push edx
		mov dx,[DMABaseAdd]
		mov al,bl
		out dx,al           ; set base address low
		mov al,bh
		out dx,al           ; set base address high

		add dl,2            ;point to length
		mov al,cl
		out dx,al           ; set length low
		mov al,ch
		out dx,al           ; set length high
		pop edx

		mov al,dl
		mov dx,[DMAPageReg]
		out dx,al           ; set DMA page reg

		mov al,[DMAChannel]
		and al,00000011b
		out 0D4h,al         ; unmask (activate) dma channel
		pop ebx
		ret

@@DoDMA03:      mov al,4
		add al,[DMAChannel]
		out 0Ah,al          ; mask reg bit

		sub al,al
		out 0Ch,al          ; clr byte ptr

		mov al,dh
		add al,[DMAChannel]
		out 0Bh,al          ; set mode reg

		push edx
		mov dx,[DMABaseAdd]
		mov al,bl
		out dx,al           ; set base address low
		mov al,bh
		out dx,al           ; set base address high

		inc dx              ;point to length
		mov al,cl
		out dx,al           ; set length low
		mov al,ch
		out dx,al           ; set length high
		pop edx

		mov al,dl
		mov dx,[DMAPageReg]
		out dx,al           ; set DMA page reg

		mov al,[DMAChannel]
		out 0Ah,al          ; unmask (activate) dma channel
		pop ebx
		ret
ProgramDMAChip  endp

;----------------------------------------------------------------------------
GetDMARegisters proc near       ;Get registers for current DMA setting
		movzx eax,[__DMAChan]
		mov [DMAChannel],al
		mov dx,[DMAAddrTable+eax*4]     ;read from look-up table
		mov [DMAPageReg],dx
		mov dx,[DMAAddrTable+eax*4+2]
		mov [DMABaseAdd],dx
		ret
GetDMARegisters endp

;----------------------------------------------------------------------------
GetDMAPage      proc near       ;return page & offset for buffer EAX
				;DL=page, AX=offset
		add eax,_code32a        ;convert EAX to physical pointer
		cmp [__DMAChan],4
		jb @noadjust
		mov edx,eax
		shr eax,1
		movzx eax,ax
		shr edx,16
		and edx,0fh
		ret
@noadjust:      mov edx,eax
		shr edx,16
		and edx,0fh
		movzx eax,ax
		ret
GetDMAPage      endp
;----------------------------------------------------------------------------
GetPageFixedBuf proc near               ;return page fixed buffer
					;ebx=total size allocated
					;edx=size of buffer to alloc
					;eax=returned address
		mov eax,[_lomembase]
		add eax,[_code32a]
		mov ecx,eax
		add ecx,edx
		shr eax,16
		shr ecx,16
		cmp ecx,eax
		jz @@nopagebound
		shl ecx,16
		sub ecx,[_code32a]
		sub ecx,[_lomembase]
		mov eax,ecx
		add ebx,eax
		call _getlomem
@@nopagebound:  mov eax,edx
		add ebx,eax
		call _getlomem
		ret
GetPageFixedBuf endp

%

;----------------------------------------------------------------------------
; Gravis Ultrasound Interface Code
;----------------------------------------------------------------------------

comment %

GUS_SetInterface proc near              ;set up DMA, IRQ to DMAChan and IrqNum
		call UltraReset
		jnc @@foundit
@@invalid_setting: stc
		ret
@@foundit:      mov bl,[__IntNum]       ;GF1 IRQ
		mov bh,0                ;MIDI IRQ
		mov cl,[__DMAChan]      ;DMA playback
		mov ch,0                ;DMA record

		and bx,0f0fh
		and cx,0707h
		mov [_GF1],bl           ; Save IRQ's
		mov [_midi],bh

		movzx edx,bl            ;****   convert IRQ numbers into register value   *****
		and dl,dl
		jz @@Zero_irq1
		mov dl,[irq_reg_list+edx]
		and dl,dl
		jz @@invalid_setting
@@Zero_irq1:    mov [_irq_control],dl

		movzx edx,bh
		and dl,dl
		jz @@Zero_irq2
		mov dl,[irq_reg_list+edx]
		and dl,dl
		jz @@invalid_setting
		shl dl,3
@@Zero_irq2:    or [_irq_control],dl

		cmp bh,bl               ; Chech if both IRQ
		jne @@diff_irqs         ;   are equal then
		and bl,bl               ;( Except when zero)
		jz @@diff_irqs
		and [_irq_control],0111b; Clear Channel 2 IRQ
		or [_irq_control],40h   ; and turn on bit 6
@@diff_irqs:
		movzx edx,cl          ;****   convert DMA number into register value   *****
		and dl,dl
		jz @@Zero_dma2
		mov dl,[dma_reg_list+edx]
		and dl,dl
		jz @@invalid_setting
@@Zero_dma1:     mov [_dma_control],dl

		movzx edx,ch
		and dl,dl
		jz @@Zero_dma2
		mov dl,[dma_reg_list+edx]
		and dl,dl
		jz @@invalid_setting
		shl dl,3
@@Zero_dma2:    or [_dma_control],dl

		cmp ch,cl               ; Chech if both DMAs
		jne @@diff_dmas           ;   are equal then
		and cl,cl               ;( Except when zero)
		jz @@diff_dmas
		and [_dma_control],0111b; Clear Channel 2 DMA
		or [_dma_control],40h   ; and turn on bit 6.
@@diff_dmas:
		cli                     ; must not be disturbed

		;The code below sets the DMA and IRQ  settings of the Ultrasound
		;It was sort of taken from the file RESET.C  of GUS SDK  V2.10

		mov ecx,200h            ; delay a bit
		loop $

		mov dx,[__PortAddr]     ;Set up for Digital ASIC
		add dx,0fh
		mov al,5
		out dx,al           ; Seems to be a undocumented register

		mov dx,[__PortAddr]
		mov al,00001011b
		out dx,al
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_CTRL
		mov al,0
		out dx,al

		mov dx,[__PortAddr]
		add dx,0fh
		out dx,al

		mov dx,[__PortAddr]     ;First do DMA control register
		mov al,00001011b
		out dx,al
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_CTRL
		mov al,[_dma_control]
		or al,80h
		out dx,al

		mov dx,[__PortAddr]     ;IRQ control register
		mov al,01001011b
		out dx,al
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_CTRL
		mov al,[_irq_control]
		out dx,al

		mov dx,[__PortAddr]     ;First do DMA control register
		mov al,00001011b
		out dx,al
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_CTRL
		mov al,[_dma_control]
		out dx,al

		mov dx,[__PortAddr]     ;IRQ control register
		mov al,01001011b
		out dx,al
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_CTRL
		mov al,[_irq_control]
		out dx,al

		mov dx,[__PortAddr]     ;IRQ CONTROL, ENABLE IRQ
		add dx,GF1_REG_SELECT   ;  just to Lock out writes to irq\dma register ...
		mov al,0
		out dx,al

		mov dx,[__PortAddr]     ;enable output & irq, disable line & mic input
		mov al,0001001b
		out dx,al

		mov dx,[__PortAddr]     ;  just to Lock out writes to irq\dma register ...
		add dx,GF1_REG_SELECT
		mov al,0
		out dx,al
		clc
		ret
GUS_SetInterface endp

;----------------------------------------------------------------------------
GUS_Init        proc near               ;Initsystem for GUS
		call UltraPing          ;ping for presence first
		jnc @@foundit
		stc
		ret
@@foundit:      mov ax,16
		call UltraReset
		call UltraDRAMSize
		mov [__DeviceRAM],eax

		mov [_DRAM_Counter],32  ;Start loading samples 32 bytes into DRAM

		;allocate a low-mem buffer for DMA sample dumping
		mov edx,1024*4
		mov [_GUS_DMABufLen],edx
		xor ebx,ebx
		call GetPageFixedBuf
		mov [_GUS_DMABuffer],eax
		mov [_GUS_LowMemTot],ebx

		;hook gus irq for dma routine
		cli
		push ebx
		mov bl,[__IntNum]               ;Grab GUS int
		call _getirqvect
		mov [Old_Int_Handler],edx
		mov bl,[__IntNum]
		mov edx,offset GUS_IRQ_Handler
		call _setirqvect
		mov bl,[__IntNum]               ;Set irq handlers
		mov edx,offset GUS_IRQ_Handler
		mov edi,offset _RealModeCode
		call _rmpmirqset
		mov [_OldRealMode],eax
		pop ebx

		mov bl,[__IntNum]       ;Get old irq mask
		call _getirqmask
		mov [_IntMask],al
		mov bl,[__IntNum]       ;Set new irq mask
		mov al,0
		call _setirqmask

		mov al,020h             ;Ack. old ints
		out 020h,al
		out 0a0h,al
		sti

		mov [__MixingRate],44100       ;set mixing rate
		mov [__Version],0               ;set version

		mov [__SoundDevice],DEVICE_GUS
		or [__SystemStatus],SYS_INIT
		clc
		ret
GUS_Init        endp

GUS_Close       proc near               ;Close system for gus
		cli
		mov al,[_IntMask]               ;Restore IRQ mask
		mov bl,[__IntNum]
		call _setirqmask
		mov edx,[Old_Int_Handler]       ;Restore IRQ handler
		mov bl,[__IntNum]
		call _setirqvect
		mov bl,[__IntNum]               ;Restore real mode IRQ handler
		mov eax,[_OldRealMode]
		call _rmpmirqfree
		mov al,020h             ;Ack. old ints
		out 020h,al
		out 0a0h,al
		sti
		mov ax,14
		call UltraReset
		mov eax,[_GUS_LowMemTot]
		sub [_lomembase],eax
		ret
GUS_Close       endp

;----------------------------------------------------------------------------
GUS_StartSong   proc near               ;Start playing module on GUS
		movzx eax,byte ptr [ebx+_TotalChanNum]
		push ebx
		call UltraReset
		pop ebx

		cmp [ebx+_SampleFlag],0
		jnz @@nodump
		;Dump samples to DRAM
		push ebx
		mov ebx,[ebx+_Pointer]
		call G_DumpSams2DRAM
		pop ebx
		mov [ebx+_SampleFlag],1
@@nodump:       mov al,[ebx+_globalvol] ;set to initial volume
		mov [ebx+_RealGlobalVol],al
		mov al,[ebx+_initialspd]
		mov [ebx+_MCurrentSpd],al
		mov al,[ebx+_initialBPM]
		mov [ebx+_MCurrentBPM],al
		mov ax,000feh
		mov [ebx+_BreakToRow],0
		mov [ebx+_MCurrentRow],al
		mov [ebx+_MRowPointer],0
		mov [ebx+_MCurrentPos],ax
		mov [ebx+_MCurrentPatt],al
		mov [ebx+_MCurrentTick],1
		mov [ebx+_MRowDelay],0
		mov [ebx+_MRowLoopStart],0
		mov [ebx+_MRowLoopCount],0

		call ClearChannels
		mov [__CurrentModule],ebx
		call LoadPanSettings

@noloadpan:     cli     ;Gus timer code
		mov al,[ebx+_MCurrentBPM]       ;Speed up timer
		call Calc_GUS_BPM
		jnc @@validBPM
		mov ax,250      ;if error, set to 125 bpm
@@validBPM:     mov [_GUS_TimerTotal],ax
		mov [_GUS_TimerLeft],ax
		call _SetGUSTimerSpd
		@outbyte [_select],TIMER_CONTROL
		@outbyte [_data_hi],04h
		mov dx,GF1_TIMER_CTRL
		add dx,[__PortAddr]
		mov al,04h
		out dx,al
		inc dx
		mov al,01h
		out dx,al
		sti
		ret
GUS_StartSong   endp

;----------------------------------------------------------------------------
Calc_GUS_BPM    proc near               ;Calc gus timer for AL=BPM
		movzx eax,al
		or al,al
		jz @@abort
		mov cx,31250
		xchg ax,cx
		xor edx,edx
		div cx
		clc
		ret
@@abort:        stc
		ret
Calc_GUS_BPM    endp

;----------------------------------------------------------------------------
_SetGUSTimerSpd proc near
		mov ax,[_GUS_TimerLeft]
		cmp ax,256
		ja @@bigBPM
		@outbyte [_select],TIMER1
		mov dx,[_data_hi]
		mov ax,256
		sub ax,[_GUS_TimerLeft]
		out dx,al
		mov [_GUS_TimerLeft],0
		ret
@@bigBPM:       ;Handle BPM < 122

		@outbyte [_select],TIMER1
		sub [_GUS_TimerLeft],256
		cmp [_GUS_TimerLeft],16
		ja @@fineness
		add [_GUS_TimerLeft],16
		@outbyte [_data_hi],16
		jmp short @@fineness2
@@fineness:     @outbyte [_data_hi],0
@@fineness2:    ret
_SetGUSTimerSpd endp

%

;----------------------------------------------------------------------------
GUS_Read_Env    proc near

		push es                 ;get ptr to environment
		mov es,[PSP_Selector]
		movzx edi,word ptr es:[2ch]
		shl edi,4
		pop es

		mov edx,offset Ultrasnd
checkvar:       mov cl,ULTRALEN
		mov ebx,edx
scanvar:        mov al,byte ptr [edi]
		cmp al,byte ptr [ebx]
		jne skipvar
		inc edi
		inc ebx
		dec cl
		jnz scanvar

		mov eax,1                   ; exit with 1, success
		xor ebx,ebx
		mov cl,3
getgusport:
		shl bx,4
		mov dl,byte ptr [edi]
		inc edi
		sub dl,'0'
		or bl,dl
		dec cl
		jnz getgusport

		mov [__PortAddr],bx
		inc edi                     ; skip past comma

getgusdma:      cmp byte ptr [edi+2],','
		je gusdma1x
		mov bl,byte ptr [edi]
		sub bl,'0'
		mov [__DMAChan],bl
		add edi,2                   ; skip past comma
		jmp skiprecdma
gusdma1x:       mov bl,byte ptr [edi+1]
		sub bl,'0'
		add bl,10
		mov [__DMAChan],bl
		add edi,3                    ; skip past comma
skiprecdma:     inc edi
		cmp byte ptr [edi],','
		jne skiprecdma
		inc edi

getgusirq:      cmp byte ptr [edi+2],','
		je gusirq1x
		mov bl,byte ptr [edi]
		sub bl,'0'
		mov [__IntNum],bl
		jmp exitgus
gusirq1x:       mov bl,byte ptr [edi+1]
		sub bl,'0'
		add bl,10
		mov [__IntNum],bl
		jmp exitgus

skipvar:        inc edi
		mov al,[edi]
		or al,al
		jnz skipvar
		inc edi
		mov al,[edi]
		or al,al
		jnz checkvar

		xor eax,eax
exitgus:        ret
GUS_Read_Env    endp

comment %

;----------------------------------------------------------------------------
GUS_StopSong    proc near               ;stop current song
		mov ax,16
		call UltraReset
		ret
GUS_StopSong    endp

;----------------------------------------------------------------------------
GUS_IRQ_Handler proc near
		pushad
		push ds es
		mov ds,cs:_seldata
		mov es,cs:_seldata

		mov dx,[__PortAddr]             ;Find source of interrupt
		add dx,GF1_IRQ_STAT
		in al,dx

@@check4vol:    test al,01100000b               ;Vol ramp or rollover irq
		jz @@check4time

		push eax
		xor eax,eax
		mov [_GUS_MaskDWord],eax

@@hvirqloop:    @outbyte [_select],GET_IRQV     ;Handle channel IRQs
		mov dx,[_data_hi]
		in al,dx

		test al,01000000b       ;check for either
		jz @@checkthem
		test al,10000000b
		jz @@checkthem
		jmp @@donevolirq

@@checkthem:    test al,01000000b       ;check for volramp
		jnz @@notvol

		push eax                        ;handle volume ramp irq
		and al,00011111b
		movzx eax,al
		mov ecx,eax
		push eax
		mov eax,1
		shl eax,cl
		test [_GUS_MaskDWord],eax
		jz @@new1
		pop eax eax
		jmp short @@notvol
@@new1:         or [_GUS_MaskDWord],eax
		pop eax
		mov ebx,[__CurrentModule]       ;Track current module
		call _GIRQStartVoice
		pop eax

@@notvol:       test al,10000000b       ;check for rollover
		jnz @@hvirqloop

		push eax                        ;handle rollover irq
		and al,00011111b
		movzx eax,al
		mov ecx,eax
		push eax
		mov eax,1
		shl eax,cl
		test [_GUS_MaskDWord],eax
		jz @@new2
		pop eax eax
		jmp @@hvirqloop
@@new2:         or [_GUS_MaskDWord],eax
		pop eax

		;process rollover
		mov dx,[__PortAddr]             ;Select the proper GUS voice
		add dx,GF1_VOICESELECT
		out dx,al

		mov ecx,ChanDataSize            ;get ptr to channeldata
		xor edx,edx
		mul ecx                         ;set for sample stop at ramp end
		;or byte ptr [ChansInfo+_ChannelFlag+eax],_CHN_StopVoice
		add eax,[__ChannelData]
		or byte ptr ds:[_ChannelFlag+eax],_CHN_StopVoice

		@outbyte [_select],SET_CONTROL  ;Change voice parameter
		@outbyte [_data_hi],0
		call GF1_Delay
		@outbyte [_select],SET_CONTROL
		@outbyte [_data_hi],0

		mov [_GUS_End],0                ;Volume off the current sample
		mov [_GUS_Mode],00100000b
		call _GRampToVol

		pop eax
		jmp @@hvirqloop

@@donevolirq:   pop eax

@@check4time:   test al,00000100b               ;Timer irq
		jz @@nomorechecks
		push ax

		@outbyte [_select],TIMER_CONTROL
		@outbyte [_data_hi],10000000b
		cmp [_GUS_TimerLeft],0          ;Set timer BPM stuff
		jnz @@noupdate
		mov ebx,[__CurrentModule]       ;Track current module
		call __UpdateTracker
		mov ebx,[__CurrentModule]       ;Track current module
		call GUS_ProcessTracks
		mov ax,[_GUS_TimerTotal]
		mov [_GUS_TimerLeft],ax

@@noupdate:     call _SetGUSTimerSpd
		@outbyte [_select],TIMER_CONTROL
		@outbyte [_data_hi],00000100b
		pop ax

@@nomorechecks: test al,128             ;DMA irq
		jz @@donechecks

		@outbyte [_select],DMA_CONTROL  ;DMA handler
		mov dx,[_data_hi]
		in al,dx
		test al,64
		jz @@donechecks
		@outbyte [_select],DMA_CONTROL
		@outbyte [_data_hi],0
		inc [_GUS_DMACount]

@@donechecks:   mov dx,[__PortAddr]             ;Find source of interrupt
		add dx,GF1_IRQ_STAT
		in al,dx
		test al,01100100b
		jz @@endint
		jmp @@check4vol
@@endint:       mov al,020h             ;Set hardware for next interrupt
		out 0A0h,al
		out 020h,al

		cmp [LoopCount],0               ;call loop handler
		jz @donehere
		mov ebx,[__CurrentModule]
		cmp [ebx+_MActualPos],0         ;wait till new patt
		jnz @donehere
		mov [LoopCount],0
		call [LoopJumpPoint]
@donehere:      pop es ds
		popad
		iretd
GUS_IRQ_Handler endp

;----------------------------------------------------------------------------
_GIRQStartVoice proc near               ;Start voices with quick vol ramp

		push eax
		mov dx,[__PortAddr]             ;Select the proper GUS voice
		add dx,GF1_VOICESELECT
		out dx,al

		@outbyte [_select],SET_VOLUME_CONTROL
		@outbyte [_data_hi],00000011b
		@outbyte [_select],SET_VOLUME_CONTROL
		@outbyte [_data_hi],00000011b
		@outbyte [_select],SET_CONTROL  ;Stop the current sample
		@outbyte [_data_hi],00000011b
		@outbyte [_select],SET_CONTROL
		@outbyte [_data_hi],00000011b

		@outbyte [_select],SET_ACC_HIGH ;Set voice ptr to zero in DRAM
		@outword [_data_low],0
		@outbyte [_select],SET_ACC_LOW
		@outword [_data_low],0

		pop eax

		mov edi,[__ChannelData]
		mov ecx,ChanDataSize
		mul ecx
		add edi,eax

		mov [edi+_ActiveFlag],0 ;zero active flag

		xor eax,eax

		test [edi+_ChannelFlag],_CHN_NewSamp
		jnz @@strtnew
		test [edi+_ChannelFlag],_CHN_StopVoice
		jnz @@checkforpan       ;@@stopsample

@@strtnew:      mov al,[edi+_SampleNum]
		cmp al,255
		je @@checkforpan        ;@@stopsample
		call PtrToSample        ;Get ptr to sample number AL
		jc @@checkforvol
		mov esi,eax

		mov [edi+_ActiveFlag],1 ;set channel active flag (info)

		mov eax,[esi+02ch]      ;Get sample in DRAM
		mov [_GUS_DramBegin],eax

		test byte ptr [esi+01fh],1
		jz @@nosampleloop
		;This for looped samples
			mov edx,[esi+014h]      ;Get sample loop start in DRAM
			add edx,eax

			mov [_GUS_DramStart],edx
			mov edx,[esi+018h]      ;Get sample loop end in DRAM
			add edx,eax

			mov [_GUS_DramEnd],edx
			mov [_GUS_Mode],00001000b
			jmp short @@gotdrampositions
@@nosampleloop: ;This for unlooped samples
			mov [_GUS_DramStart],eax
			mov edx,[esi+010h]
			add edx,eax

			mov eax,[esi+010h]      ;<--- rollover modify
			cmp eax,64*2
			jb @nochange
			mov eax,64*2
@nochange:              sub edx,eax
			mov [_GUS_DramEnd],edx
			mov [_GUS_Mode],00100000b
@@gotdrampositions:
		movzx eax,word ptr [edi+_SampleOffset] ;(account for sample offset)
		add [_GUS_DramBegin],eax

		test byte ptr [esi+01fh],4      ;set 16-bit flag
		jz @@not16bit
		or [_GUS_Mode],00000100b
		push ebx
		mov ebx,[_GUS_DramBegin]
		call _convertto16bit
		mov [_GUS_DramBegin],ebx
		mov ebx,[_GUS_DramStart]
		call _convertto16bit
		mov [_GUS_DramStart],ebx
		mov ebx,[_GUS_DramEnd]
		call _convertto16bit
		mov [_GUS_DramEnd],ebx
		pop ebx

@@not16bit:     ;Set start loop address of buffer
		@outbyte [_select],SET_START_HIGH
		mov eax,[_GUS_DramStart]
		@ADDRHIGH
		@outword [_data_low],ax
		@outbyte [_select],SET_START_LOW
		mov eax,[_GUS_DramStart]
		@ADDRLOW
		@outword [_data_low],ax

		;Set end address of buffer
		@outbyte [_select],SET_END_HIGH
		mov eax,[_GUS_DramEnd]
		@ADDRHIGH
		@outword [_data_low],ax
		@outbyte [_select],SET_END_LOW
		mov eax,[_GUS_DramEnd]
		@ADDRLOW
		@outword [_data_low],ax

		@outbyte [_select],SET_ACC_HIGH ;Set accumulator to beginning of data
		mov eax,[_GUS_DramBegin]        ;  (do this x2 if no stop sample)
		@ADDRHIGH
		@outword [_data_low],ax
		@outbyte [_select],SET_ACC_LOW
		mov eax,[_GUS_DramBegin]
		@ADDRLOW
		@outword [_data_low],ax

		;Start playing voice
		@outbyte [_select],SET_CONTROL
		@outbyte [_data_hi],[_GUS_Mode]
		call GF1_Delay
		@outbyte [_select],SET_CONTROL
		@outbyte [_data_hi],[_GUS_Mode]

		jmp short @@dovol

@@checkforvol:  test [edi+_ChannelFlag],_CHN_NewVol
		jz @@checkforpitch
@@dovol:        mov al,[edi+_ActualVol]
		mov ah,[ebx+_RealGlobalVol]
		call FetchGUSVolume
		shr ax,4
		mov [_GUS_End],ax

		cmp [_GUS_Mode],00001000b       ;<--- rollover modify
		jz @@loopedvol
		mov [_GUS_Mode],VC_ROLLOVER
		jmp short @@outvol
@@loopedvol:    mov [_GUS_Mode],00000000b
@@outvol:       call _GRampToVol

@@checkforpitch: test [edi+_ChannelFlag],_CHN_NewPitch
		jz @@checkforpan
		@outbyte [_select],SET_FREQUENCY        ;Set frequency
		mov eax,[edi+_ActualPeriod]             ;Convert frequency

		call PeriodToPitch
		jz @@doneprocess
		call FetchGUSFreq
		@outword [_data_low],ax

@@checkforpan:  test [edi+_ChannelFlag],_CHN_NewPan
		jz @@doneprocess                ;Handle new pan setting
		@outbyte [_select],SET_BALANCE
		mov dx,[_data_hi]
		mov al,[edi+_PanPosition]
		out dx,al
@@doneprocess:  mov [edi+_ChannelFlag],0
		ret
_GIRQStartVoice endp

;----------------------------------------------------------------------------
_convertto16bit proc near       ;convert address for 16-bit samples/dma
				;convert adderss in EBX
		mov eax,ebx             ;convert_to_16bit()
		shr ebx,1
		and ebx,0001ffffh
		and eax,000c0000h
		or ebx,eax
		ret
_convertto16bit endp

;----------------------------------------------------------------------------
GUS_ProcessTracks proc near     ;Device specific output code
		;Assume all register offset variables are already calculated
		;  (via UltraReset)

		xor ecx,ecx
		mov edi,[__ChannelData]
		;Now: EBX=Module header, EDI=Channel Ptr, ESI=sample ptr
		;CL=Voice count

@@ProcessLoop:  push ecx
		cmp [edi+_ChannelFlag],0
		je @@doneprocess

		mov dx,[__PortAddr]             ;Select the proper GUS voice
		add dx,GF1_VOICESELECT
		mov al,cl  ;[edi+_ChannelNumber]
		out dx,al

@@checkforsamp: test [edi+_ChannelFlag],_CHN_NewSamp
		jz @@checkforvol

		mov [_GUS_End],0                ;Stop the current sample
		mov [_GUS_Mode],00100000b
		call _GRampToVol
		jmp @@checkforbpm

@@checkforvol:  test [edi+_ChannelFlag],_CHN_NewVol
		jz @@checkforpitch
		and [edi+_ChannelFlag],_ICHN_NewVol
		mov al,[edi+_ActualVol]
		mov ah,[ebx+_RealGlobalVol]
		call FetchGUSVolume
		shr ax,4
		mov [_GUS_End],ax
		mov [_GUS_Mode],00000000b
		call _GRampToVol

@@checkforpitch: test [edi+_ChannelFlag],_CHN_NewPitch
		jz @@checkforpan
		and [edi+_ChannelFlag],_ICHN_NewPitch
		@outbyte [_select],SET_FREQUENCY        ;Set frequency
		mov eax,[edi+_ActualPeriod]             ;Convert frequency

		call PeriodToPitch
		jz @@doneprocess
		call FetchGUSFreq
		@outword [_data_low],ax

@@checkforpan:  test [edi+_ChannelFlag],_CHN_NewPan
		jz @@checkforbpm                ;Handle new pan setting

		and [edi+_ChannelFlag],_ICHN_NewPan
		@outbyte [_select],SET_BALANCE
		mov dx,[_data_hi]
		mov al,[edi+_PanPosition]
		out dx,al

@@checkforbpm:  test [edi+_ChannelFlag],_CHN_NewBPM
		jz @@doneprocess                ;Handle new bpm setting

		and [edi+_ChannelFlag],_ICHN_NewBPM
		movzx eax,byte ptr [ebx+_MCurrentBPM] ;Speed up timer
		call Calc_GUS_BPM
		jnc @@validBPM
		mov ax,250      ;if error, set to 125 bpm
@@validBPM:     mov [_GUS_TimerTotal],ax
		mov [_GUS_TimerLeft],ax
		call _SetGUSTimerSpd

@@doneprocess:  pop ecx
		add edi,ChanDataSize
		inc cl
		cmp cl,[ebx+_TotalChanNum]
		jb @@ProcessLoop
		ret
GUS_ProcessTracks endp

;----------------------------------------------------------------------------
_GRampToVol     proc near               ;Ramp witout ramp irq

		;@outbyte [_select],SET_VOLUME_RATE
		;@outbyte [_data_hi],63  ;01h

		@outbyte [_select],GET_VOLUME   ;Determine current volume
		mov dx,[_data_low]
		in ax,dx
		shr ax,4
		mov [_GUS_Start],ax
		mov cx,ax               ;keep original start value
		and [_GUS_Mode],10111111b
		cmp ax,[_GUS_End]       ;flip start & end if decreasing numbers ...
		jbe @@dontswap
		mov ax,[_GUS_End]
		mov [_GUS_Start],ax
		mov [_GUS_End],cx
		or [_GUS_Mode],01000000b
@@dontswap:     cmp [_GUS_Start],64
		jae @@nofixstart
		mov [_GUS_Start],64
@@nofixstart:   cmp [_GUS_End],4032
		jbe @@nofixend
		mov [_GUS_End],4032
@@nofixend:     @outbyte [_select],SET_VOLUME_START
		mov ax,[_GUS_Start]
		shr ax,4
		mov dx,[_data_hi]
		out dx,al
		@outbyte [_select],SET_VOLUME_END
		mov ax,[_GUS_End]
		shr ax,4
		mov dx,[_data_hi]
		out dx,al
		shl cx,4        ;Also MUST set the current volume to the start volume ...
		@outbyte [_select],SET_VOLUME
		@outword [_data_low],cx
		@outbyte [_select],SET_VOLUME_CONTROL   ;start 'er up !!!
		@outbyte [_data_hi],[_GUS_Mode]
		call GF1_Delay
		@outbyte [_select],SET_VOLUME_CONTROL
		@outbyte [_data_hi],[_GUS_Mode]
		ret
_GRampToVol     endp

;----------------------------------------------------------------------------
FetchGUSFreq    proc near       ;Returns GUS freq value from EAX
		push ecx

		shl eax,09h
		movzx ecx,word ptr [_GUS_DivisorValue]
		shr ecx,1
		add eax,ecx
		mov cx,[_GUS_DivisorValue]
		xor edx,edx

		div ecx
		add ax,ax
		pop ecx

		ret
FetchGUSFreq    endp

FetchGUSVolume  proc near       ;Returns GUS volume for AL in AX
		push ebx
		mov bl,ah
		mul bl          ;real vol = (g.vol*c.vol)/64
		shr ax,6
		cmp al,64
		jbe @@novolprobs
		mov al,64
@@novolprobs:   movzx ebx,al
		add ebx,ebx
		add ebx,offset Volume_Table
		mov ax,ds:[ebx]
		pop ebx
		ret
FetchGUSVolume  endp

;----------------------------------------------------------------------------
G_DumpSams2DRAM proc near               ;Read module at ESI and dump all samples to dram

		push esi edi
		mov esi,ebx

		movzx ecx,word ptr [esi+22h]

		movzx eax,word ptr [esi+20h]
		add eax,ebx
		add eax,60h
		mov edi,eax             ;ESI=Ptr to module, EDI=Ptr to current inst offset

@@dumploop:     movzx ebx,word ptr [edi]
		shl ebx,4
		inc edi
		inc edi
		add ebx,esi

		call G_DumpS3MSample

		dec ecx
		jnz @@dumploop

		pop edi esi
		ret
G_DumpSams2DRAM endp

;----------------------------------------------------------------------------
G_DumpS3MSample proc near               ;Dump S3M type sample at EBX
					;ESI=Ptr to module
		push esi ecx ebx ebp
		mov ecx,[ebx+10h]
		or ecx,ecx
		jz @@abort              ;ECX=Num of bytes to move

		mov ebp,ebx

		mov eax,[_DRAM_Counter]
		mov [ebx+02ch],eax

		push eax
		add eax,ecx             ;Set counter for next sample
		inc eax                 ;  <-- add extra byte for anti-click
		shr eax,5               ;round number to 32-byte boundaries
		inc eax
		shl eax,5
		mov [_DRAM_Counter],eax

		movzx eax,word ptr [ebx+0eh]
		shl eax,4
		add eax,esi
		pop ebx

		push edi
		push esi                ;EBX=Dest in GUS DRAM, ESI=Src in RAM
		mov esi,eax             ;CX=Num of bytes to move

		mov eax,[ebp+10h]

		mov edi,ebx             ;set paste point at end of samp
		add edi,eax

		test byte ptr [ebp+01fh],1
		jz @@nosamloop          ;set paste point at loopend
		mov eax,[ebp+18h]
		mov edi,ebx
		add edi,eax
		mov eax,[ebp+14h]

@@nosamloop:    mov al,[esi+eax]
		xor al,128
		push edi eax
		call GUS_DumpSamLoop
		pop eax edi
		pop esi
		push ebx
		mov ebx,edi
		cli
		call UltraPokeByte      ;Copy byte to fix loop clicks
		sti
		pop ebx
		pop edi

		inc ebx

@@exit:         pop ebp ebx ecx esi
		ret
@@abort:        mov dword ptr [ebx+02ch],0
		jmp @@exit

G_DumpS3MSample endp

;----------------------------------------------------------------------------
GUS_DumpSamLoop proc near               ;Sample dump loop (GUS)
					;DMA xfer or direct
		cmp [__DMAFlag],1
		jz @DMAxfer

		test byte ptr [ebp+01fh],4      ;adjust for 16-bit samps
		jz @@copyloop
@@copyloop16:   lodsb                   ;dont use DMA sample dump
		;xor al,128             ;16-bit sample dump
		cli
		call UltraPokeByte
		sti
		inc ebx
		dec ecx
		jnz @@copyloop16
		ret
@@copyloop:     lodsb                   ;dont use DMA sample dump
		xor al,128
		cli
		call UltraPokeByte
		sti
		inc ebx
		dec ecx
		jnz @@copyloop
		ret

@DMAxfer:       call GetDMARegisters    ;ESI=Source in system ram
		mov eax,[_GUS_DMABuffer];ECX=Bytes to copy
		call GetDMAPage         ;EBX=Pointer to dest in DRAM
		mov [DMA_Page],dl
		mov [DMA_Offset],ax

		push ecx
		push ebx
		mov [Temp_Counter],ecx
		mov [Temp_Pointer],ebx

		;main copy and dump loop
@@dmadumploop:  mov ecx,[_GUS_DMABufLen]        ;ESI=ptr to sample data
		and cl,11100000b
		mov ebx,[Temp_Pointer]          ;EBX=pointer to DRAM
		mov eax,[Temp_Counter]          ;ECX=length of buffer to dump
		cmp eax,ecx
		jae @nobuffix
		mov ecx,[Temp_Counter]

@nobuffix:      mov eax,[Temp_Pointer]          ;check for 256k DRAM boundaries
		mov edx,eax
		and edx,11111111111111100000000000000000b
		add eax,ecx
		and eax,11111111111111100000000000000000b
		cmp eax,edx
		jz @fine
		sub eax,[Temp_Pointer]          ;fix for boundary
		and al,11100000b
		mov ecx,eax

@fine:          sub [Temp_Counter],ecx
		add [Temp_Pointer],ecx

		push ecx                        ;copy data to lo-mem buffer
		mov edi,[_GUS_DMABuffer]
		rep movsb
		pop ecx

		mov dh,48h                      ;program DMAC for xfer
		mov dl,[DMA_Page]
		mov ax,[DMA_Offset]
		push ecx
		dec ecx
		call ProgramDMAChip
		pop ecx
		call ProgramGUSDMA

		sti
@@waitfordma:   hlt
		cmp [_GUS_DMACount],0           ;wait for dma to finish
		jz @@waitfordma

		cmp [Temp_Counter],0
		jnz @@dmadumploop
		pop ebx
		pop ecx
		add ebx,ecx
		ret
GUS_DumpSamLoop endp

;----------------------------------------------------------------------------
ProgramGUSDMA   proc near               ;GUS DMA start code
		mov [Temp_Mode],10100001b

		test byte ptr [ebp+01fh],4      ;adjust for 16-bit samps
		jz @@not16bit
		or [Temp_Mode],01000000b
		and [Temp_Mode],01111111b
@@not16bit:
		cmp [__DMAChan],4
		jb @@lowdma
		or [Temp_Mode],00000100b ;set for 16-bit dma channel
		call _convertto16bit

@@lowdma:       cli
		@outbyte [_select],SET_DMA_ADDRESS ;set dram address
		mov dx,[_data_low]
		mov eax,ebx
		shr eax,4
		out dx,ax
		@outbyte [_select],DMA_CONTROL  ;start DMA xfer
		@outbyte [_data_hi],[Temp_Mode]
		mov [_GUS_DMACount],0
		sti
		ret
ProgramGUSDMA   endp

;----------------------------------------------------------------------------
UltraReset      proc near               ;Fully reset GUS card with AX voices

		cmp ax,32               ;Check for valid number of voices
		jbe @@notmore            ; ie. 14<=voices<=32
		stc
		ret
@@notmore:      cmp ax,14
		jae @@notless
		mov ax,14
@@notless:      mov [_GUS_NumVoices],ax

		movzx eax,ax            ;Precalculate frequency divisor
		mov ax,[offset Divisor_Table-28+eax*2]

		mov [_GUS_DivisorValue],ax

		xor ebx,ebx             ;Zero first two locations of dram
		xor eax,eax
		call UltraPokeWord

		cli                     ;Prevent system interrupts

		mov dx,[__PortAddr]
		add dx,GF1_REG_SELECT
		mov [_select],dx
		inc dx
		mov [_data_low],dx
		inc dx
		mov [_data_hi],dx

		@outbyte [_select],MASTER_RESET ;Pull a reset on the GF1
		@outbyte [_data_hi],0

		mov cl,10                       ;Wait a little while ...
@@lppt1:        call GF1_Delay
		dec cl
		jnz @@lppt1

		@outbyte [_select],MASTER_RESET ;Release Reset
		@outbyte [_data_hi],GF1_MASTER_RESET

		mov cl,10                       ;Wait a little while ...
@@lppt2:        call GF1_Delay
		dec cl
		jnz @@lppt2

		mov dx,[__PortAddr]     ;Reset the MIDI port also
		add dx,GF1_MIDI_CTRL
		@outb MIDI_RESET
		mov cl,10               ;Wait a little while ...
@@lppt3:        call GF1_Delay
		dec cl
		jnz @@lppt3
		xor al,al
		out dx,al

		@outbyte [_select],DMA_CONTROL  ;Clear all interrupts.
		@outbyte [_data_hi],0
		@outbyte [_select],TIMER_CONTROL
		@outbyte [_data_hi],0
		@outbyte [_select],SAMPLE_CONTROL
		@outbyte [_data_hi],0

		;Set the number of active voices
		@outbyte [_select],SET_VOICES
		mov dx,[_data_hi]
		mov ax,[_GUS_NumVoices]
		dec al
		or al,0c0h
		out dx,al

		;Clear interrupts on voices.
		;Reading the status ports will clear the irqs.
		mov dx,[__PortAddr]
		add dx,GF1_IRQ_STAT
		in al,dx
		@outbyte [_select],DMA_CONTROL
		mov dx,[_data_hi]
		in al,dx
		@outbyte [_select],SAMPLE_CONTROL
		mov dx,[_data_hi]
		in al,dx
		@outbyte [_select],GET_IRQV
		mov dx,[_data_hi]
		in al,dx

		;Loop to program each voice
		xor ecx,ecx
@@progvoicelp:  mov dx,[__PortAddr]             ;Select the proper voice
		add dx,GF1_VOICESELECT
		mov eax,ecx
		out dx,al

		;Stop the voice and volume
		@outbyte [_select],SET_CONTROL
		@outbyte [_data_hi],00000011b
		@outbyte [_select],SET_VOLUME_CONTROL
		@outbyte [_data_hi],00000011b

		call GF1_Delay                  ;Wait 4.8 micos. or more.

		;Initialize each voice specific registers.
		@outbyte [_select],SET_FREQUENCY
		@outword [_data_low],0400h
		@outbyte [_select],SET_START_HIGH
		@outword [_data_low],0
		@outbyte [_select],SET_START_LOW
		@outword [_data_low],0
		@outbyte [_select],SET_END_HIGH
		@outword [_data_low],0
		@outbyte [_select],SET_END_LOW
		@outword [_data_low],0
		@outbyte [_select],SET_VOLUME_RATE
		@outbyte [_data_hi],63          ;01h
		@outbyte [_select],SET_VOLUME_START
		@outbyte [_data_hi],010h
		@outbyte [_select],SET_VOLUME_END
		@outbyte [_data_hi],0e0h
		@outbyte [_select],SET_VOLUME
		@outword [_data_low],0

		@outbyte [_select],SET_ACC_HIGH
		@outword [_data_low],0
		@outbyte [_select],SET_ACC_LOW
		@outword [_data_low],0
		@outbyte [_select],SET_BALANCE
		@outbyte [_data_hi],07h
		inc ecx
		cmp cx,[_GUS_NumVoices]
		jb @@progvoicelp

		mov dx,[__PortAddr]
		add dx,GF1_IRQ_STAT
		in al,dx
		@outbyte [_select],DMA_CONTROL
		mov dx,[_data_hi]
		in al,dx
		@outbyte [_select],SAMPLE_CONTROL
		mov dx,[_data_hi]
		in al,dx
		@outbyte [_select],GET_IRQV
		mov dx,[_data_hi]
		in al,dx

		;Set up GF1 Chip for interrupts & enable DACs.
		@outbyte [_select],MASTER_RESET
		@outbyte [_data_hi],GF1_MASTER_RESET+GF1_OUTPUT_ENABLE+GF1_MASTER_IRQ

		;mov cl,10                       ;Wait a little while ...
@@lppt4:        ;call GF1_Delay
		;dec cl
		;jnz @@lppt4

		;Enable output in mixer
		@outbyte [__PortAddr],9
		sti

		call UltraMixProbe              ;determine if ICS mixer present
		or al,al
		jz @@done
		call UltraMixXparent            ;make ICS mixer inactive
@@done:         ret
UltraReset      endp

UltraMixProbe   proc near                       ;return mixer revision number of board
						;AL=rev. number or 0 for no mixer
		mov dx,[__PortAddr]
		add dx,MIX_SEL_PORT
		in al,dx

		cmp al,5                ;if ((val >= 5 && val <=7) || (val >= 0x81 && val <= 0x90))
		jb @@nextor
		cmp al,7
		jbe @@clearmix
@@nextor:       cmp al,081h
		jb @@zero
		cmp al,090h
		ja @@zero
@@clearmix:     mov [_mix_addr],dx      ;get mixer port addresses
		mov dx,[__PortAddr]
		add dx,MIX_DATA_PORT
		mov [_mix_data],dx
		mov [_mix_revision],al
		ret
@@zero:         xor al,al
		mov [_mix_revision],al
		ret
UltraMixProbe   endp

UltraMixXparent proc near                       ;make ICS mixer transparent
		xor ebx,ebx
@@xloop:        call UltraMixAttn       ;fix all chans
		inc bl
		cmp bl,6
		jz @@xdone
		cmp bl,4
		jb @@xloop
		inc bl
		jmp @@xloop
@@xdone:        ret
UltraMixXparent endp

_ctrl_addr      db      ?
_attn_addr      db      ?
_normal         db      ?

_flip_left      db      1,1,1,2,1,2
_flip_right     db      2,2,2,1,2,1

UltraMixAttn    proc near               ;clear ICS mixer channel BL
		push ebx                ;clears both left and right
		shl bl,3
		mov [_ctrl_addr],bl     ;handle left channel first
		mov [_attn_addr],bl
		or [_ctrl_addr],MIX_CTRL_LEFT
		or [_attn_addr],MIX_ATTN_LEFT
		mov [_normal],01h
		cmp [_mix_revision],FLIP_REV
		jnz @@noflip1
		pop ebx
		push ebx
		mov al,[_flip_left+ebx]
		mov [_normal],al
@@noflip1:      call @@setRLchan
		pop ebx

		push ebx
		shl bl,3
		mov [_ctrl_addr],bl     ;handle right channel
		mov [_attn_addr],bl
		or [_ctrl_addr],MIX_CTRL_RIGHT
		or [_attn_addr],MIX_ATTN_RIGHT
		mov [_normal],02h
		cmp [_mix_revision],FLIP_REV
		jnz @@noflip2
		pop ebx
		push ebx
		mov al,[_flip_right+ebx]
		mov [_normal],al
@@noflip2:      call @@setRLchan
		pop ebx
		ret
@@setRLchan:    cli
		@outbyte [_mix_addr],[_ctrl_addr]
		@outbyte [_mix_data],[_normal]
		@outbyte [_mix_addr],[_attn_addr]
		@outbyte [_mix_data],07fh
		sti
		ret
UltraMixAttn    endp

;----------------------------------------------------------------------------
GF1_Delay       proc near               ;GUS pause for card
		push eax edx
		mov ah,7
		mov dx,[__PortAddr]
		add dx,GF1_DRAM
@@inloop:       in al,dx
		dec ah
		jnz @@inloop
		pop edx eax
		ret
GF1_Delay       endp

;----------------------------------------------------------------------------
UltraPing       proc near               ;Test for card, NC=Card found
		xor ebx,ebx             ;Read old value
		call UltraPeekWord
		push eax

		mov al,0aah             ;Set temp check DRAM variables
		call UltraPokeByte
		inc ebx
		mov al,055h
		call UltraPokeByte
		dec ebx

		call UltraPeekByte
		mov cl,al

		pop eax                 ;Restore old value
		xor ebx,ebx
		call UltraPokeWord

		cmp cl,0aah
		jnz @@error
		clc
		ret
@@error:        stc
		ret
UltraPing       endp

;----------------------------------------------------------------------------
UltraPeekByte   proc near                       ;Read a byte from DRAM at EBX
		push ebx
		push ecx
		mov ecx,ebx                     ;CX=LSW,BX=MSW
		shr ebx,16

		mov dx,[__PortAddr]             ; get port
		add dx,103h                     ; point to dram port

		mov al,043h
		out dx,al                       ; point to dram low register
		inc dx                          ; point to data low
		mov ax,cx                       ; get lsw part of addr
		out dx,ax                       ; send low part of address

		dec dx                          ; back to register select
		mov al,044h
		out dx,al                       ; point to dram high register
		inc dx
		inc dx                          ; point to data high register
		mov ax,bx                       ; get msw part of addr
		out dx,al                       ; send low byte off address

		inc dx
		inc dx                          ; point to dram data
		in al,dx                        ; read data
		pop ecx
		pop ebx
		ret
UltraPeekByte   endp

;----------------------------------------------------------------------------
UltraPeekWord   proc near                       ;Read a word from DRAM at EBX
		push ebx
		push ecx
		mov ecx,ebx                     ;CX=LSW,BX=MSW
		shr ebx,16

		mov dx,[__PortAddr]             ; get port
		add dx,103h                     ; point to dram port

		mov al,043h
		out dx,al                       ; point to dram low register
		inc dx                          ; point to data low
		mov ax,cx                       ; get lsw part of addr
		out dx,ax                       ; send low part of address

		dec dx                          ; back to register select
		mov al,044h
		out dx,al                       ; point to dram high register
		inc dx
		inc dx                          ; point to data high register
		mov ax,bx                       ; get msw part of addr
		out dx,al                       ; send low byte off address

		inc dx                          ; point to dram data
		in ax,dx                        ; read data
		pop ecx
		pop ebx
		ret
UltraPeekWord   endp

;----------------------------------------------------------------------------
UltraPokeByte   proc near                       ;Poke byte AL to EBX in DRAM
		push ebx
		push ecx
		push eax
		mov dx,[__PortAddr]             ; get port
		add dx,103h                     ; point to register select

		mov ecx,ebx                     ;CX=LSW, BX=MSW
		shr ebx,16

		mov al,043h
		out dx,al                       ; point to dram low register
		inc dx                          ; point to data low
		mov ax,cx
		out dx,ax                       ; send low part of address

		dec dx                          ; back to register select
		mov al,044h
		out dx,al                       ; point to dram high register
		inc dx
		inc dx                          ; point to data high register
		mov al,bl
		out dx,al                       ; send low byte off address

		inc dx
		inc dx                          ; point to dram data
		pop eax
		out dx,al                       ; write value
		pop ecx
		pop ebx
		ret
UltraPokeByte   endp

;----------------------------------------------------------------------------
UltraPokeWord   proc near                       ;Poke word AX to EBX in DRAM
		push ebx
		push ecx
		push eax
		mov dx,[__PortAddr]             ; get port
		add dx,103h                     ; point to register select

		mov ecx,ebx                     ;CX=LSW, BX=MSW
		shr ebx,16

		mov al,043h
		out dx,al                       ; point to dram low register
		inc dx                          ; point to data low
		mov ax,cx
		out dx,ax                       ; send low part of address

		dec dx                          ; back to register select
		mov al,044h
		out dx,al                       ; point to dram high register
		inc dx
		inc dx                          ; point to data high register
		mov al,bl
		out dx,al                       ; send low byte off address

		inc dx                          ; point to dram data
		pop eax
		out dx,ax                       ; write value
		pop ecx
		pop ebx
		ret
UltraPokeWord   endp

;----------------------------------------------------------------------------
UltraDRAMSize   proc near               ;Return size of GUS dram in EAX
		push ebx
		xor ebx,ebx
@@seekloop:     call UltraPeekByte
		push eax
		mov al,0cch
		call UltraPokeByte
		xor al,al
		call UltraPeekByte
		cmp al,0cch
		jne @@foundmaxr
		pop eax
		call UltraPokeByte
		add ebx,256*1024
		cmp ebx,1024*1024
		jb @@seekloop
		jmp short @@foundmax
@@foundmaxr:    pop eax
		call UltraPokeByte
@@foundmax:     mov eax,ebx
		pop ebx
		ret
UltraDRAMSize   endp
;----------------------------------------------------------------------------

;----------------------------------------------------------------------------
; Sound Blaster Interface Code
;----------------------------------------------------------------------------
SB_Init         proc near               ;Initsystem for SB
					;mixing rate in BX
					;buffer size in DX
		call SB_ResetMono
		jnc @@fine
		ret

@@fine:         call SB_GetDSPVersion

		push dx
		call SetSBMixingRate    ;set mixing rate to BX
		mov [__MixingRate],ax
		pop dx

		shr dx,4                ;round down to nearest 16bytes
		shl dx,4
		mov [_SB_BufferSize],dx ;allocate DMA buffers
		movzx edx,dx
		xor ebx,ebx

		call GetPageFixedBuf
		mov [_SB_Buffer_0],eax
		mov edi,eax             ;clear buffer
		movzx ecx,dx
		mov al,80h
		rep stosb

		call GetPageFixedBuf
		mov [_SB_Buffer_1],eax
		mov edi,eax             ;clear buffer
		movzx ecx,dx
		mov al,80h
		rep stosb

		mov [_SB_LowMemTotal],ebx

		;allocate hi-mem mixing buffers
		xor ebx,ebx
		call InitMixingTables
		movzx eax,[_SB_BufferSize]
		add eax,eax
		add ebx,eax
		call _gethimem
		mov [MixingTable],eax
		mov [_SB_HiMemTotal],ebx

		call _lomemsize         ;get free ram
		mov edx,eax
		call _himemsize
		add eax,edx
		mov [__DeviceRAM],eax   ;set free ram size

		cli
		mov bl,[__IntNum]               ;Grab SB irq
		call _getirqvect
		mov [Old_Int_Handler],edx
		mov bl,[__IntNum]
		mov edx,offset SB_IRQ_Handler
		call _setirqvect
		mov bl,[__IntNum]               ;Set irq handlers
		mov edx,offset SB_IRQ_Handler
		mov edi,offset _RealModeCode
		call _rmpmirqset
		mov [_OldRealMode],eax
		mov bl,[__IntNum]               ;Get old irq mask
		call _getirqmask
		mov [_IntMask],al
		mov bl,[__IntNum]               ;Set new irq mask
		mov al,0
		call _setirqmask
		mov al,020h                     ;Ack. old ints
		out 020h,al
		out 0a0h,al
		sti

		mov [_SB_CurrentBuf],1          ;start dma system
		call GetDMARegisters
		call SwapSBBuffers

		mov [__SoundDevice],DEVICE_SBMONO
		or [__SystemStatus],SYS_INIT
		clc
		ret
SB_Init         endp

SB_GetDSPVersion proc near              ;determine DSP version
		push ax bx dx
		mov dx,[__PortAddr]
		add dx,0ch              ;port 2xCh

@@pause1:       in al,dx                ;wait for SB
		or al,al
		js @@pause1
		mov al,0e1h             ;select DSP version register
		out dx,al

		add dx,2                ;port 2xEh      - get high byte
@@pause2:       in al,dx
		or al,al
		jns @@pause2
		sub dx,4                ;port 2xAh
		in al,dx
		mov bh,al

		add dx,4                ;port 2xEh      - get low byte
@@pause3:       in al,dx
		or al,al
		jns @@pause3
		sub dx,4                ;port 2xAh
		in al,dx
		mov bl,al

		mov [__Version],bx      ;set version number
		pop dx bx ax
		ret
SB_GetDSPVersion endp

InitMixingTables proc near              ;Create mixing tables

		mov eax,[_himembase]    ;make sure 256 byte aligned
		shr eax,8
		inc eax
		shl eax,8
		sub eax,[_himembase]
		add ebx,eax
		call _gethimem
		mov eax,65*256
		add ebx,eax
		call _gethimem          ;allocate 65*256 for volumetable
		mov [VolumeTable],eax

		push ebx

		mov edi,eax
		xor bx,bx               ;let bx=volume
		xor cx,cx               ;    cx=data
@@lp1:          mov al,cl               ;creation loop
		sub ax,128
		imul bl
		sar ax,6
		stosb
		inc cl
		jnz @@lp1
		inc bl
		cmp bl,65
		jb @@lp1

		pop ebx

		mov eax,2048
		add ebx,eax
		call _gethimem          ;allocate 2048 for posttable
		mov [PostTable],eax
		mov edi,eax
		mov ecx,2048
		mov ax,128
		rep stosb
		ret
InitMixingTables endp

;----------------------------------------------------------------------------
SB_StartSong    proc near               ;Start SB with song header at EBX

		mov cl,[ebx+_mastervol] ;create post-processing table
		and cx,127
		cmp cx,10h              ;make sure master vol is within limits
		jae @@fine
		mov cx,10h
@@fine:         mov ax,2048*16
		xor dx,dx
		div cx                  ;ax=c
		mov dx,2048
		sub dx,ax
		shr dx,1                ;dx=a

		push bx
		mov bx,ax               ;bx=c
		add bx,dx

		mov edi,[PostTable]
		mov cx,0
@@makepostlp:   mov al,0                 ;check x < a
		cmp cx,dx
		jle @@setbyte
		mov al,255               ;check x > b
		cmp cx,bx
		jge @@setbyte
		push edx ecx ebx
		mov ax,cx
		sub ax,dx
		movsx eax,ax
		sub bx,dx
		movsx ebx,bx
		sal eax,8
		cdq
		idiv ebx
		pop ebx ecx edx
@@setbyte:      stosb
		inc cx
		cmp cx,1024*2
		jl @@makepostlp
		pop bx


		mov al,[ebx+_globalvol] ;set to initial volume
		mov [ebx+_RealGlobalVol],al
		mov al,[ebx+_initialspd]
		mov [ebx+_MCurrentSpd],al
		mov al,[ebx+_initialBPM]
		mov [ebx+_MCurrentBPM],al
		mov ax,000feh
		mov [ebx+_BreakToRow],0
		mov [ebx+_MCurrentRow],al
		mov [ebx+_MRowPointer],0
		mov [ebx+_MCurrentPos],ax
		mov [ebx+_MCurrentPatt],al
		mov [ebx+_MCurrentTick],1
		mov [ebx+_MRowDelay],0
		mov [ebx+_MRowLoopStart],0
		mov [ebx+_MRowLoopCount],0

		call ClearChannels
		mov [__CurrentModule],ebx
		call LoadPanSettings

		mov edi,offset SB_Tables        ;clear channel table
		mov ecx,Mix_Size*32
		xor eax,eax
		rep stosb

		call SetSBTempo
		xor eax,eax
		mov [_SB_GapCount],eax
		ret
SB_StartSong    endp

SetSBTempo      proc near               ;set BPM for SB
		movzx eax,[__MixingRate]        ;calc bytes / update
		mov edx,10
		mul edx
		movzx ecx,[ebx+_MCurrentBPM]
		xor edx,edx
		div ecx
		shr eax,2                       ;EAX = bytes to mix
		mov [_SB_GapLength],eax
		ret
SetSBTempo      endp

;----------------------------------------------------------------------------
SB_IRQ_Handler  proc near               ;SB IRQ handler
		pushad
		push ds es
		mov ds,cs:_seldata
		mov es,cs:_seldata

		mov dx,[__PortAddr]     ;Ak. SB interrupt
		add dx,0eh
		in al,dx
		call SwapSBBuffers
		mov al,020h             ;Set hardware for next interrupt
		out 0A0h,al
		out 020h,al
		;sti

		test [__SystemStatus],SYS_PLAY
		jz @@notplaying

		mov ebx,[__CurrentModule]       ;EBX = ptr to current module
		mov al,[ebx+_TotalChanNum]
		mov [_SB_NumChans],al

		movzx eax,[_SB_BufferSize]      ;ecx = bytes remaining to fill
		mov [_SB_BufCount],eax
		mov edi,[_SB_Buffer_1]          ;edi = ptr to buffer
		cmp [_SB_CurrentBuf],0
		jz @@fine
		mov edi,[_SB_Buffer_0]

@@fine:         cmp [_SB_GapCount],0
		ja @domix
		push edi
		mov ebx,[__CurrentModule]       ;EBX = ptr to current module
		call __UpdateTracker
		mov ebx,[__CurrentModule]       ;EBX = ptr to current module
		call SB_ProcessTracks
		pop edi
		mov eax,[_SB_GapLength]
		mov [_SB_GapCount],eax
@domix:         mov ecx,[_SB_GapCount]
		cmp ecx,[_SB_BufCount]
		jb @@fine2
		mov ecx,[_SB_BufCount]
@@fine2:        call Mixer_8bitMono
		add edi,ecx
		sub [_SB_GapCount],ecx
		sub [_SB_BufCount],ecx
		jnz @@fine

		;loop to set channel active flags (info)
		mov ebx,[__CurrentModule]
		mov cl,[ebx+_TotalChanNum]
		mov edi,[__ChannelData]
		mov esi,offset SB_Tables
@@infoloop:     mov al,[esi+_Mix_ActiveFlag]
		mov [edi+_ActiveFlag],al
		add edi,ChanDataSize
		add esi,Mix_Size
		dec cl
		jnz @@infoloop

		cmp [LoopCount],0               ;call loop handler
		jz @@notplaying
		mov ebx,[__CurrentModule]
		cmp [ebx+_MActualPos],0         ;wait till new patt
		jnz @@notplaying
		mov [LoopCount],0
		call [LoopJumpPoint]

@@notplaying:   pop es ds
		popad
		iretd
SB_IRQ_Handler  endp

;----------------------------------------------------------------------------
SB_ProcessTracks proc near               ;process channel data
		xor cx,cx
		mov edi,[__ChannelData]
		;Now: EBX=Module header, EDI=Channel Ptr, ESI=sample ptr
		;CL=Voice count
		;EBP=ptr to SB channel data

		mov ebp,offset ds:SB_Tables

@@ProcessLoop:  push cx
		cmp [edi+_ChannelFlag],0
		je @@doneprocess

@@checkforsamp: test [edi+_ChannelFlag],_CHN_NewSamp
		jz @@checkforvol

		mov al,[edi+_SampleNum]          ;start new sample
		cmp al,255
		jne @@dontstop
@@stop:         xor eax,eax
		mov ds:[ebp+_Mix_CurrentPtr],eax
		mov ds:[ebp+_Mix_LoopLen],eax
		mov ds:[ebp+_Mix_LoopEnd],eax
		mov ds:[ebp+_Mix_Volume],eax
		mov ds:[ebp+_Mix_ActiveFlag],al
		jmp @@checkforpitch

@@dontstop:     call PtrToSample
		jc @@stop

		mov esi,eax
		movzx edx,word ptr [esi+0eh]      ;set start of sample
		shl edx,4
		add edx,[ebx+_Pointer]
		mov ds:[ebp+_Mix_CurrentPtr],edx
		movzx eax,[edi+_SampleOffset]   ;add sample offset
		add ds:[ebp+_Mix_CurrentPtr],eax
		mov ds:[ebp+_Mix_ActiveFlag],1  ;activate sample

		dec edx                 ;ajust for actual sample end

		test byte ptr [esi+1fh],1
		jz @@nolooping

		mov eax,[esi+18h]
		sub eax,[esi+14h]
		mov ds:[ebp+_Mix_LoopLen],eax
		mov eax,[esi+18h]
		add eax,edx
		mov ds:[ebp+_Mix_LoopEnd],eax   ;set loop end
		mov eax,[ebp+_Mix_CurrentPtr]
@@trynow:       cmp eax,[ebp+_Mix_LoopEnd]      ;adjust for offset past end
		jbe @@setsample
		sub eax,[ebp+_Mix_LoopLen]
		mov ds:[ebp+_Mix_CurrentPtr],eax
		jmp @@trynow
@@nolooping:    mov ds:[ebp+_Mix_LoopLen],0
		add edx,[esi+10h]
		mov ds:[ebp+_Mix_LoopEnd],edx      ;set sample end point
@@setsample:    mov ds:[ebp+_Mix_Count],0

@@checkforvol:  test [edi+_ChannelFlag],_CHN_NewVol
		jz @@checkforpitch

		mov al,[edi+_ActualVol]         ;set new volume
		mov ah,[ebx+_RealGlobalVol]

		push ebx
		mov bl,ah
		mul bl          ;real vol = (g.vol*c.vol)/64
		shr ax,6
		cmp ax,64
		jb @@noclipv
		mov ax,64
@@noclipv:      pop ebx
		shl ax,8
		movzx eax,ax
		mov ds:[ebp+_Mix_Volume],eax

@@checkforpitch: test [edi+_ChannelFlag],_CHN_NewPitch
		jz @@checkforpan

		mov eax,[edi+_ActualPeriod]     ;set new pitch
		call PeriodToPitch
		jz @@doneprocess

		movzx ecx,[__MixingRate]        ;calc step rates
		xor edx,edx
		div ecx
		mov ds:[ebp+_Mix_HighSpeed],eax
		xor eax,eax
		shrd eax,edx,16
		div ecx
		xor ax,ax
		mov ds:[ebp+_Mix_LowSpeed],eax

		shr eax,16
		mov ecx,[ebp+_Mix_HighSpeed]
		shl ecx,16
		or ecx,eax
		xor eax,eax
		mov edx,1
		or ecx,ecx
		jz @@checkforpan
		div ecx
		mov ds:[ebp+_Mix_ScaleRate],eax

@@checkforpan:  test [edi+_ChannelFlag],_CHN_NewPan
		jz @@checkforbpm                ;Handle new pan setting

		mov al,[edi+_PanPosition]       ;set new pan position
		shl al,3
		mov ds:[ebp+_Mix_PanPos],al

@@checkforbpm:  test [edi+_ChannelFlag],_CHN_NewBPM
		jz @@doneprocess                ;Handle new bpm setting

		call SetSBTempo         ;set new BPM

@@doneprocess:  mov [edi+_ChannelFlag],0
		pop cx
		add edi,ChanDataSize
		add ebp,Mix_Size
		inc cl
		cmp cl,[ebx+_TotalChanNum]
		jb @@ProcessLoop
		ret
SB_ProcessTracks endp

;----------------------------------------------------------------------------

_Mix8bitMono    macro                   ;mix 1 byte of 1 channel
		add eax,edx
		mov bl,[ebp]
		adc ebp,esi
		movsx ax,byte ptr [ebx]
		add [edi],ax
		add edi,2
endm

Mixer_8bitMono  proc near               ;mix 8-bit mono data

		push ecx edi            ;mix ECX bytes to buffer EDI
		push edi
		mov [TempMixCount],ecx

		mov edi,[MixingTable]           ;clear pre-mixing table
		movzx ecx,[_SB_BufferSize]
		mov eax,04000400h               ;1024
		shr ecx,1
		rep stosd

		mov esi,offset SB_Tables
		mov dl,[_SB_NumChans]

@@mixloop:      mov edi,[MixingTable]
		mov ecx,[TempMixCount]

		push edx

		cmp [esi+_Mix_ActiveFlag],0
		jz @@donemixing

		cmp [esi+_Mix_LoopLen],0
		jz @@nolooping

@@handleloopedb: mov edx,ecx                     ;mix with looping
@@handlelooped: mov eax,[esi+_Mix_LoopEnd]
		sub eax,[esi+_Mix_CurrentPtr]
		jg @@noloopnow
@@notzeroyet:   mov eax,[esi+_Mix_CurrentPtr]   ;adjust for loop end
		sub eax,[esi+_Mix_LoopLen]
		mov [esi+_Mix_CurrentPtr],eax
		jmp short @@handlelooped
@@noloopnow:
		;convert eax remaining to actual bytes
		push edx
		mul [esi+_Mix_ScaleRate]
		or ax,ax
		jz @@noaddbyte
		add eax,10000h
@@noaddbyte:    shrd eax,edx,16
		pop edx

		cmp eax,ecx
		ja @@noajustloop
		mov ecx,eax
@@noajustloop:  sub edx,ecx
		or ecx,ecx
		jnz @@cxnotzero
		inc ecx
@@cxnotzero:    push edx esi
		mov eax,[esi+_Mix_Count]
		mov edx,[esi+_Mix_LowSpeed]
		mov ebp,[esi+_Mix_CurrentPtr]
		mov ebx,[VolumeTable]
		add ebx,[esi+_Mix_Volume]
		mov esi,[esi+_Mix_HighSpeed]

		push cx
		shr cx,4
		push cx
		jz @@no16bytes
@@16byteloop:   rept 16                 ;unroll loop 16 times
		_Mix8bitMono
		endm
		dec cx
		jnz @@16byteloop
@@no16bytes:    pop ax
		pop cx
		shl ax,4
		sub cx,ax
		jz @@donetheloop
@@looploop:     _Mix8bitMono
		dec cx
		jnz @@looploop
@@donetheloop:  pop esi
		mov [esi+_Mix_CurrentPtr],ebp
		;xor ax,ax
		mov [esi+_Mix_Count],eax
		pop ecx
		or ecx,ecx
		jg @@handleloopedb
		jmp @@donemixing

@@nolooping:    mov eax,[esi+_Mix_LoopEnd]      ;mix without looping
		sub eax,[esi+_Mix_CurrentPtr]   ;check remaining bytes
		jle @@stopsample

		;convert eax remaining to actual bytes
		mul [esi+_Mix_ScaleRate]
		shrd eax,edx,16

		cmp eax,ecx
		ja @@noajustend                 ;limit to remaining bytes
		mov ecx,eax
		mov [esi+_Mix_ActiveFlag],0     ;last time
@@noajustend:   or ecx,ecx
		jz @@stopsample
		push esi

		mov eax,[esi+_Mix_Count]
		mov edx,[esi+_Mix_LowSpeed]
		mov ebp,[esi+_Mix_CurrentPtr]
		mov ebx,[VolumeTable]
		add ebx,[esi+_Mix_Volume]
		mov esi,[esi+_Mix_HighSpeed]

		push cx
		shr cx,4
		push cx
		jz @@no16bytes2
@@16byteloop2:  rept 16                 ;unroll loop 16 times
		_Mix8bitMono
		endm
		dec cx
		jnz @@16byteloop2
@@no16bytes2:   pop ax
		pop cx
		shl ax,4
		sub cx,ax
		jz @@donetheloop2
@@nolooploop2:  _Mix8bitMono
		dec ecx
		jnz @@nolooploop2
@@donetheloop2: pop esi
		mov [esi+_Mix_CurrentPtr],ebp
		;xor ax,ax
		mov [esi+_Mix_Count],eax
		jmp short @@donemixing
@@stopsample:   mov [esi+_Mix_ActiveFlag],0

@@donemixing:   pop edx
		add esi,Mix_Size
		dec dl
		jnz @@mixloop

		pop edi

		;copy mixed table through post-table to EDI
		mov ecx,[TempMixCount]
		mov esi,[MixingTable]
		mov edx,[PostTable]
@@copytable:
		movsx ebx,word ptr [esi]
		mov al,[ebx+edx]

		mov [edi],al
		inc esi
		inc edi
		inc esi
		dec ecx
		jnz @@copytable

		pop edi ecx
		ret

Mixer_8bitMono  endp

;----------------------------------------------------------------------------
SB_StopSong     proc near
		mov edi,[_SB_Buffer_0]  ;clear buffers
		movzx ecx,[_SB_BufferSize]
		mov al,80h
		rep stosb
		mov edi,[_SB_Buffer_1]
		movzx ecx,[_SB_BufferSize]
		rep stosb
		mov edi,offset SB_Tables
		mov ecx,Mix_Size*32/4
		xor eax,eax
		rep stosd
		ret
SB_StopSong     endp

;----------------------------------------------------------------------------
ProgramSBChip   proc near               ;set SB to output CX bytes
		dec cx
		cmp [__MixingRate],21739
		jbe @@slowdma

		mov dx,[__PortAddr]     ;use fast DMA transfer
		add dx,0ch
@@outloop1:     in al,dx                ;wait for sb
		or al,al
		js @@outloop1
		mov al,48h              ;send hi-speed buffer length
		out dx,al
		call @@outbuff2         ;send buffer length bytes
@@outloop2:     in al,dx                ;wait for sb
		or al,al
		js @@outloop2
		mov al,91h              ;send hi-speed start
		out dx,al
		ret

@@slowdma:      mov dx,[__PortAddr]     ;use slow DMA transfer
		add dx,0ch
@@outbuff1:     in al,dx
		or al,al
		js @@outbuff1
		mov al,14h
		out dx,al                       ;send buffer length
@@outbuff2:     in al,dx
		or al,al
		js @@outbuff2
		mov al,cl
		out dx,al
@@outbuff3:     in al,dx
		or al,al
		js @@outbuff3
		mov al,ch
		out dx,al
		ret
ProgramSBChip   endp

;----------------------------------------------------------------------------
SwapSBBuffers   proc near               ;flip dma buffers
		cmp [_SB_CurrentBuf],0
		jz @@setto1
		mov [_SB_CurrentBuf],0
		mov eax,[_SB_Buffer_0]          ;program DMAC for xfer
		jmp short @@setit
@@setto1:       mov [_SB_CurrentBuf],1
		mov eax,[_SB_Buffer_1]          ;program DMAC for xfer
@@setit:        call GetDMAPage
		mov dh,48h              ;48h
		mov cx,[_SB_BufferSize]
		call ProgramDMAChip
		call SetTimeConstant
		mov cx,[_SB_BufferSize]         ;program DSP for xfer
		dec cx
		call ProgramSBChip
		ret
SwapSBBuffers   endp

;----------------------------------------------------------------------------
SetSBMixingRate proc near               ;set mixing rate closest to BX
		movzx ebx,bx
		or ebx,ebx
		jz @@abort

		cmp [__Version],201h    ;dependant on DSP version
		jb @@oldcard
		cmp bx,21739            ;if < 22khz, use slow DMA
		jbe @@oldcard

		mov eax,256000000       ;calc high speed time constant
		xor edx,edx
		div ebx
		mov edx,65536
		sub edx,eax
		shr edx,8
		cmp edx,233             ;limit at 43478hz
		jb @@fine2
		mov edx,233
@@fine2:        mov [_SB_TimeConst],dl
		shl edx,8               ;return actual rate in EAX
		mov ebx,65536
		sub ebx,edx
		mov eax,256000000
		xor edx,edx
		div ebx
		clc
		ret

@@oldcard:      mov eax,1000000
		xor edx,edx
		div ebx
		mov edx,256
		sub edx,eax             ;edx=time constant
		cmp edx,210
		jb @@fine
		mov edx,210             ;limit to 21739hz
@@fine:         mov eax,edx
		mov [_SB_TimeConst],al

		mov ebx,256             ;return mixing rate in EAX
		sub ebx,eax
		or ebx,ebx
		jz @@abort
		mov eax,1000000
		xor edx,edx
		div ebx
		clc
		ret
@@abort:        stc
		ret
SetSBMixingRate endp

;----------------------------------------------------------------------------
SetTimeConstant proc near
		mov dx,[__PortAddr]
		add dx,0ch              ;select port 2xCh
@@outbuff:      in al,dx
		or al,al
		js @@outbuff
		mov al,40h
		out dx,al               ;write 40h
@@outbuff2:     in al,dx
		or al,al
		js @@outbuff2
		mov al,[_SB_TimeConst]
		out dx,al               ;write time constant
		ret
SetTimeConstant endp

;----------------------------------------------------------------------------
SB_ResetMono    proc near               ;reset SB card

		push ax cx dx
		mov dx,[__PortAddr]     ;write 01 to 2x6h
		add dx,06h
		mov al,01h
		out dx,al
		mov ecx,010000h
@@sbresetlp:    loop @@sbresetlp        ;wait 3 microsecs
		mov al,0
		out dx,al               ;write 00 to 2x6h

		add dx,4                ;select port 2xAh
		mov ecx,0100h
@@sbtestloop:   add dx,4
		in al,dx
		sub dx,4
		or al,al
		jns @@sbtestloop
		in al,dx
		cmp al,0aah
		jz @@sbreseted
		loop @@sbtestloop       ;abort if no response
		stc
		jmp @@leave

@@sbreseted:    add dx,2                ;select port 2xCh
@@pause:        in al,dx
		or al,al
		js @@pause
		mov al,0d1h             ;enable DAC output
		out dx,al
		clc
@@leave:        pop dx cx ax
		ret
SB_ResetMono    endp

;----------------------------------------------------------------------------
SB_Close        proc near               ;Close system for SB
		cli
		mov al,[_IntMask]               ;Restore IRQ mask
		mov bl,[__IntNum]
		call _setirqmask
		mov edx,[Old_Int_Handler]       ;Restore IRQ handler
		mov bl,[__IntNum]
		call _setirqvect
		mov bl,[__IntNum]               ;Restore real mode IRQ handler
		mov eax,[_OldRealMode]
		call _rmpmirqfree
		call SB_ResetMono
		mov al,020h                     ;Ack. old ints
		out 020h,al
		out 0a0h,al
		sti

		mov eax,[_SB_LowMemTotal]       ;free lo-mem dma buffers
		sub [_lomembase],eax

		mov eax,[_SB_HiMemTotal]        ;free hi-mem mixing buffers
		sub [_himembase],eax

		ret
SB_Close        endp

;----------------------------------------------------------------------------
SB_Read_Env     proc near               ;Get SB details from env
		push es
		push gs
		pop es

		xor eax,eax
		mov edi,_pspa
		mov ax,gs:[edi+2ch]
		shl eax,4
		mov edi,eax

		mov edx,offset Blaster
@checkvar:      mov cl,BLASTERLEN
		mov ebx,edx
@scanvar:       mov al,byte ptr es:[edi]
		cmp al,byte ptr [ebx]
		jne @skipvar
		inc edi
		inc ebx
		dec cl
		jnz @scanvar

@nextchar:      mov al,es:[edi]                 ;check for 'A'
		inc edi
		or al,al
		jz @@abort
		cmp al,'A'
		jnz @nextchar

		xor bx,bx
		mov cl,3
@getsbport:     shl bx,4
		mov dl,byte ptr es:[edi]
		inc edi
		sub dl,'0'
		or bl,dl
		dec cl
		jnz @getsbport
		mov [__PortAddr],bx

@nextcharI:     mov al,es:[edi]                 ;check for 'I'
		inc edi
		or al,al
		jz @@abort
		cmp al,'I'
		jnz @nextcharI

		xor bx,bx
		mov bl,es:[edi]
		inc edi
		sub bl,'0'
		cmp byte ptr es:[edi],'0'
		jb @getsbdma
		cmp byte ptr es:[edi],'9'
		ja @getsbdma
		mov al,10
		mul bl
		mov bl,es:[edi]
		add bx,ax
		inc edi

@getsbdma:      mov [__IntNum],bl

@findbigD:      mov al,es:[edi]                 ;check for 'D'
		inc edi
		or al,al
		jz @@abort
		cmp al,'D'
		jnz @findbigD

		mov bl,es:[edi]
		sub bl,'0'
		mov [__DMAChan],bl

		mov ax,1
		jmp short @exitsb

@skipvar:       inc edi
		mov al,es:[edi]
		or al,al
		jnz @skipvar
		inc edi
		mov al,es:[edi]
		or al,al
		jnz @checkvar
@@abort:        xor ax,ax
@exitsb:        pop es
		ret
SB_Read_Env     endp

;----------------------------------------------------------------------------

%

_TEXT           ends
		end
