; STARPLAYER for PMODE/W
;----------------------------------------------------------------------------
		.386p
		locals

_TEXT           segment use32 dword public 'CODE'
		assume  cs:_TEXT,ds:_DATA


start:          jmp short _main

		db 'WATCOM'             ; The "WATCOM" string is needed in
                                        ; order to run under DOS/4G and WD.

		include cdromlib.inc

;----------------------------------------------------------------------------
; CODE
;----------------------------------------------------------------------------

;----------------------------------------------------------------------------
; Entry To ASM Code (_main)
; In:
;   CS - Code Selector    Base: 00000000h - Limit: 4G
;   DS - Data Selector    Base: 00000000h - Limit: 4G
;   ES - PSP Selector     Base: PSP Seg   - Limit: 100h
;   FS - ?
;   GS - ?
;   SS - Data Selector    Base: 00000000h - Limit: 4G
;   ESP -> STACK segment
;   Direction Flag - ?
;   Interrupt Flag - ?
;
;   All Other Registers Are Undefined!
;----------------------------------------------------------------------------
_main:          sti                     ; Set The Interrupt Flag
		cld                     ; Clear The Direction Flag

		mov [PSP_Selector],es   ;set up selectors
		mov ax,ds
		mov es,ax
		mov fs,ax
		mov gs,ax

		call DoCDThings

nogood:         mov ax,4c00h            ; AH=4Ch - Exit To DOS
		int 21h                 ; DOS INT 21h

;----------------------------------------------------------------------------
DoCDThings      proc near

		call CD_Installed

		jc @@installed
		mov edx,offset Msg_2
		mov ah,9
		int 21h
		ret
@@installed:

		movzx eax,[CDROM_STATS._drives]
		call Bin2Dec
		mov edi,0b8000h+23*160
		call @putdec2
		movzx eax,[CDROM_STATS._first_drive]
		call Bin2Dec
		call @putdec2

		movzx eax,[CDROM_STATS._disk_length_min]
		call Bin2Dec
		call @putdec2


		mov eax,10
		call PlayCDSlot
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



DoCDThings      endp


PlayCDSlot      proc near               ;start song AL, single flag in AH
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

@@ret:          ret
PlayCDSlot      endp


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

_TEXT           ends

;----------------------------------------------------------------------------
; DATA
;----------------------------------------------------------------------------
_DATA           segment use32 dword public 'DATA'


		extrn CDROM_STATS:NEAR

PSP_Selector    dw      ?

Msg_1           db      'CD-ROM extensions detected.',10,13,'$'
Msg_2           db      'CD-ROM extensions NOT detected.',10,13,'$'

asciitbl        db      10 DUP(?)

;----------------------------------------------------------------------------

CDSTEPTIME      equ     10      ;time to fast forward / rewind cd

CDROM_Mode      db      ?       ;Set this when in CD audio mode
CDROM_Flag      db      ?       ;Set this if CDROM is installed
CDROM_Playing   db      ?       ;set if cdrom is playing
CDROM_Paused    db      ?       ;set if cdrom is paused
CDROM_Done      db      ?       ;set if cdrom is done
CDROM_StatusFlag dd     ?       ;flag for cdrom abilities

CDVolume        CD_Volumeinfo   ?

CDWasOpenFlag   db      ?       ;set this if cd tray is opened

CDPlayStatus    CD_Playinfo     ?

_DATA           ends

;----------------------------------------------------------------------------
; STACK
;----------------------------------------------------------------------------
stack           segment para stack 'STACK'
		db 1000h dup(?)
stack           ends

        end start

