; STARPLAYER for PMODE/W
;----------------------------------------------------------------------------
		.386p
		locals

_TEXT           segment use32 dword public 'CODE'
		assume  cs:_TEXT,ds:_DATA

		public _lomemsize
		public _himemsize
		public _gethimem
		public _freehimem

		public PSP_Selector

start:          jmp short _main

		db 'WATCOM'             ; The "WATCOM" string is needed in
                                        ; order to run under DOS/4G and WD.

		include s3mlib.inc
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

		call ParseCMDLine       ;read cmd line parameters
		jc nogood

		push es
		mov ah,02fh             ;get current DTA address
		int 21h
		mov [DTASeg],es
		mov [DTAOff],ebx
		pop es

		mov dl,0
		mov eax,M_Struc_Size*MAXSLOTS
		call _gethimem          ;get hi-mem for song slot headers
		jc abortprog2
		mov [H_SlotTable],ebx   ;  save handle
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
		mov [H_ScreenBuffer],ebx

		mov eax,80*50*2         ;get hi-mem for menu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer2],eax
		mov [H_ScreenBuffer2],ebx

		mov eax,80*50*2         ;get hi-mem for filemenu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer3],eax
		mov [H_ScreenBuffer3],ebx

		mov eax,80*50*2         ;get hi-mem for another menu buffer
		call _gethimem
		jc abortprog2
		mov [ScreenBuffer4],eax
		mov [H_ScreenBuffer4],ebx

		mov eax,EXTRADATASIZE   ;get hi-mem for temp menu stuff
		call _gethimem
		jc abortprog2
		mov [ExtraData],eax
		mov [H_ExtraData],ebx

		mov eax,FileDataSize*MAXLISTSIZE
		call _gethimem          ;get hi-mem for file-list
		jc abortprog2
		mov [FileList],eax
		mov [H_FileList],ebx

		mov eax,768             ;get mem for palette
		call _gethimem
		jc abortprog2
		mov [Temp_Palette],eax
		mov [H_Temp_Palette],ebx

		mov eax,LISTSIZEMAX             ;get hi-mem for list buffers
		mov [ListFileFree],eax
		call _gethimem
		jc abortprog2
		mov [ListFileBuffer],eax
		mov [H_ListFileBuffer],ebx

@@nochange:     mov eax,3                       ;Use bios to set videomode
		int 10h

		call SetScreenSize

		;call HookKeyboard
		mov [ProtectFlag],0

		mov al,[ForceCard]
		mov bx,[Default_MixRate]
		mov dx,[Default_BufLen]
		call PM_InitSystem
		mov dl,1
		jc abortprog

		call PrepareScreen

		;call EndScreen

nogood:         mov ax,4c00h            ; AH=4Ch - Exit To DOS
		int 21h                 ; DOS INT 21h

abortprog:      push edx
		;call FreeKeyboard
		pop edx
abortprog2:     push edx
		call EndScreen
		mov edx,offset ErrorMsg         ;display error msgs
		mov ah,9
		int 21h
		pop edx
		movzx eax,dl
		mov edx,[ErrorTable+eax*4]
		mov ah,9
		int 21h
		jmp nogood

;----------------------------------------------------------------------------
ParseCMDLine    proc near
		push es                 ;Parse command line parameters
		mov es,[PSP_Selector]
		mov edi,81h
		mov esi,offset Filename

@findspclp:     mov al,es:[edi]
		inc edi
		cmp al,13
		jz @@nocmdline
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
		jz @@nocmdline
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
		stc
		ret
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

@@nocmdline:    pop es
		clc
		ret
ParseCMDLine    endp

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
ShowCmdLineHelp proc near
		mov edx,offset Init_Msg0
		mov ah,9
		int 21h
		mov edx,offset CmdLne_Help1
		mov ah,9
		int 21h
		ret
ShowCmdLineHelp endp

;----------------------------------------------------------------------------
SetScreenSize   proc near       ;set screen size
		mov ax,00083h                   ;Reset videomode
		int 10h
		mov [Scr_Ypos],24
		mov al,[Last_Row]
		cmp al,50
		jb @@donehere
		mov ax,1112h             ;Set small fonts
		mov bx,0
		int 10h
		mov [Scr_Ypos],49
		mov [Last_Row],50
		ret
@@donehere:     mov [Last_Row],25
		ret
SetScreenSize   endp

;----------------------------------------------------------------------------
PrepareScreen   proc near

		mov dx,3dah             ;wait for vertical retrace
@@VRT:          in al,dx
		test al,8
		jnz @@VRT               ;wait until Vertical Retrace starts
@@NoVRT:        in al,dx
		test al,8
		jz @@NoVRT              ;wait until Vertical Retrace Ends

		;;call DisableKeyboard
		;mov bl,1                ;Set new irq mask
		;mov al,1
		;call _setirqmask

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

		mov ebp,[ScreenBuffer]
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

		mov ebp,[ScreenBuffer]
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
		;call DrawVUBars                 ;Show VU-Bars

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
		mov ebp,[ScreenBuffer]
		call WriteStr

@@nosong:       mov edi,0b8000h                ;copy header to screen
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
		mov edi,0b8000h+(160*4)        ;blank channel bars on screen
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
@@donehere:     ;;call EnableKeyboard
		;mov bl,1                ;Set new irq mask
		;mov al,0
		;call _setirqmask

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
		;sub edi,_code32a
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
_himemsize      proc near               ;return free high mem in EAX
		mov ax,0500h            ;  extended mem
		push edi
		mov edi,offset _MemStats
		int 31h
		jc nogood
		mov eax,dword ptr [_MemStats]
		pop edi
		ret
_himemsize      endp

_lomemsize      proc near               ;return free low mem in EAX
		mov ah,48h              ;  get conventional
		push ebx
		mov bx,0ffffh
		int 21h
		mov eax,ebx
		shl eax,4
		pop ebx
		ret
_lomemsize      endp

_gethimem       proc near               ;get hi mem
					;EAX=size of buffer required
					;returns: EAX=ptr to buffer,
					;         EBX=handle for memory block
		push ecx esi edi edx
		mov ebx,eax             ;allocate himem block
		mov ecx,eax
		shr ebx,16
		mov ax,0501h
		int 31h
		jc @@done
		shl ebx,16
		mov eax,ebx
		mov ax,cx
		shl esi,16
		mov ebx,esi
		mov bx,di
		clc
@@done:         pop edx edi esi ecx
		ret
_gethimem       endp

_freehimem      proc near               ;free hi-mem block EAX
		push eax esi edi
		mov edi,eax
		shr eax,16
		mov esi,eax
		mov ax,0502h
		int 31h
		pop edi esi eax
		ret
_freehimem      endp

;----------------------------------------------------------------------------
Bin2Dec         proc near               ;Convert EAX to decimal
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
		pop edi edx ecx ebx eax
		ret
Bin2Dec         endp

;----------------------------------------------------------------------------
EndScreen       proc near
		mov ax,3                 ;Use bios to set videomode
		int 10h
		call DrawStarAnsi

		mov edi,0b8000h+(9*160) ;Draw contacts
		mov ecx,ContactAnsi_Len
		mov esi,offset ContactAnsi
		call DeCrunch

		mov ax,0200h            ;Set cursor pos
		mov dx,1400h
		mov bx,0
		int 10h

		ret
EndScreen       endp

;----------------------------------------------------------------------------
DrawStarAnsi    proc near
		mov edi,0b8000h
		mov ecx,EndAnsi_Len     ;Draw log off ansi
		mov esi,offset EndAnsi
		call DeCrunch
		ret
DrawStarAnsi    endp

;----------------------------------------------------------------------------
DeCrunch        proc near               ;unpack thedraw crunched text
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

@Done:          pop edi edx ecx ebx eax
		ret
DeCrunch        endp

;----------------------------------------------------------------------------
_TEXT           ends

;----------------------------------------------------------------------------
; DATA
;----------------------------------------------------------------------------
_DATA           segment use32 dword public 'DATA'

		extrn CDROM_STATS:NEAR

PSP_Selector    dw      ?

_MemStats       db      48 dup(?)       ;temp for _himemsize

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
H_Temp_Palette  dd      ?       ;handle for palettebuffer

H_SlotTable     dd      ?       ;handle for song slot list

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
H_ListFileBuffer dd     ?       ;handle for listfile buffers

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
DTAOff          dd      ?

ExtraOffset     dd      ?               ;value to skip rows for scroll routine
ChangedFlag     dd      ?               ;flag for song state (byte diff)
ScreenBuffer    dd      ?               ;pointer to temporary screen buffer
ScreenBuffer2   dd      ?               ;pointer to menu screen buffer
ScreenBuffer3   dd      ?               ;pointer to file screen buffer
ScreenBuffer4   dd      ?               ;pointer to another menu buffer
ExtraData       dd      ?               ;pointer to temp himem buffer for menus
EXTRADATASIZE   equ     65+3            ;size of extra menu data

H_ScreenBuffer  dd      ?               ;handle for temporary screen buffer
H_ScreenBuffer2 dd      ?               ;handle for menu screen buffer
H_ScreenBuffer3 dd      ?               ;handle for file screen buffer
H_ScreenBuffer4 dd      ?               ;handle for another menu buffer
H_ExtraData     dd      ?               ;handle for temp himem buffer for menus

Init_Msg0       db      '-+ starplayer version 2.25 +-',0,'$'
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
ErrorMsg0       db      'Not enough free extended memory.',13,10,'$'
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
H_FileList      dd      ?       ;handle for file-list in mem
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
EndAnsi_Len     equ 514-17
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


ContactAnsi_Len equ 422-15
ContactAnsi     label byte
	db      1,16,'Û',25,'MÛ',24,'Û  ',14,'contacting the author '
	db      '(jedi/oxygen) -',25,'&',1,'Û',24,'Û',25,'MÛ',24,'Û '
	db      ' ',12,'if you have internet access, mail:',25,5,'sn'
	db      'ail mail: ',7,'scott mcnab',25,12,1,'Û',24,'Û',25,8,7
	db      'jedi@tartarus.uwa.edu.au',25,20,'5 honeydew cl',25,10
	db      1,'Û',24,'Û  ',12,'look at the oxygen web page:',25,23
	db      7,'maida vale wa 6057',25,5,1,'Û',24,'Û',25,8,7,'htt'
	db      'p://www.it.com.au/oxygen/',25,16,'australia',25,14,1
	db      'Û',24,'Û  ',12,'the official oxygen ftp site:',25,'.'
	db      1,'Û',24,'Û',25,8,7,'ftp://peace.wit.com/kosmic/oxyg'
	db      'en/',25,'"',1,'Û',24,'Û',25,'MÛ',24,'Û',26,3,'Ü ',26
	db      5,'Ü ÜÜÜ ÜÜ Ü Ü',25,2,'ÜÜ Ü Ü ',26,3,'Ü ',26,5,'Ü ',26
	db      '!ÜÛ'

HeadLine_Len    equ 117-18-6
HeadLine        label byte
	db      1,16,'°',0,17,'±° ',15,'-+ starplayer +- ',3,'protec'
	db      'ted mode ',10,'multi-module ',3,'player  ',15,'(c) '
	db      'jedi / oxygen - ',0,'°±',1,16,'°',24

HeaderAnsi_Len  equ 154-6-17
HeaderAnsi      label byte
	db      1,23,' ',4,'memory ',0,'Ú ',4,'conventional',0,':',25
	db      13,4,'title',0,':',25,'$',24,25,7,'³ ',4,'extra',0,':'
	db      25,18,4,'pattern',0,':',25,9,4,'spd',0,':',25,5,4,'v'
	db      'olume',0,':',25,9,24,25,7,'À ',4,'soundcard',0,':',25
	db      18,4,'row',0,':',25,9,4,'bpm',0,':',25,7,4,'song',0,':'
	db      25,9

CDStatusLine_Len equ 180-17-6 ;184-18-5
CDStatusLine    label byte
	db      0,23,' ',4,'memory ',0,'Ú ',4,'conventional',0,':',25
	db      14,4,'ú cd audio mode ú',25,9,'volume',0,':',25,7,24,25
	db      7,'³ ',4,'extra',0,':',25,20,4,'track',0,':',25,8,4,'t'
	db      'ime',0,':',25,7,4,'remain',0,':',25,7,24,25,7,'À ',4
	db      'soundcard',0,':',25,15,4,'status',0,':',25,7,4,'tot'
	db      'al',0,':',25,7,4,'remain',0,':',25,7

InfoAnsi_Len    equ 111-25
InfoAnsi        label byte
	db      0,23,' Ú',26,5,'Ä ',4,'sample name ',0,26,5,'Ä¿  ',4,'#'
	db      '  vol. ',0,'ÚÄÄÄ ',4,'vu bar ',0,'ÄÄÄ¿ ',4,'pan ',0,'Ú'
	db      'ÄÄÄ ',4,'current fx ',0,'ÄÄÄ¿ '

Menu_1_Len      equ 173-22
Menu_1          label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º  ',14,'slot',25,4,'úùú module  titles úùú',25,6,'s'
	db      'ize',25,3,'chan samps  len  patts  ',11,'º',25,2,24,25
	db      2,8,'º  ÚÄÄ',11,'¿ ',8,'Ú',26,27,'Ä',11,'¿ ',8,'Ú',26
	db      6,'Ä',11,'¿ ',8,'ÚÄÄ',11,'¿ ',8,'ÚÄÄÄ',11,'¿ ',8,'ÚÄ'
	db      'ÄÄ',11,'¿ ',8,'ÚÄÄÄ',11,'¿  º',25,2

Menu_2_Len      equ 87-25
Menu_2          label byte
	db      0,19,25,2,8,'º  ³  ',11,'³ ',8,'³',16,25,27,11,19,'³'
	db      ' ',8,'³',25,6,11,'³ ',8,'³  ',11,'³ ',8,'³',25,2,11,'³'
	db      ' ',8,'³',25,2,11,'³ ',8,'³',25,2,11,'³  º',25,2

Menu_3_Len      equ 201-20  ;201-20
Menu_3          label byte
	db      8,19,25,2,'º  À',11,'ÄÄÙ ',8,'À',11,26,27,'ÄÙ ',8,'À'
	db      11,26,6,'ÄÙ ',8,'À',11,'ÄÄÙ ',8,'À',11,'ÄÄÄÙ ',8,'À',11
	db      'ÄÄÄÙ ',8,'À',11,'ÄÄÄÙ  º',25,2,24,25,2,8,'È',11,26,'G'
	db      'Í¼',25,2,24,' Ú',26,'KÄ',8,'¿ ',24,' ',11,'³ L/TAB='
	db      'load C=cdmode P=play S=stop V=view F=free R=release'
	db      ' W=write ESC=quit ',8,'³ ',24,' ',11,'À',8,26,'KÄÙ '
	db      24,25,'O'

Filemenu_1_Len  equ 196-17-2
Filemenu_1      label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º',25,6,'Ú',26,'7Ä',11,'¿',25,6,'º',25,2,24,25,2,8,'º'
	db      25,2,14,'path',8,'³',25,'7',11,'³',25,6,'º',25,2,24,25
	db      2,8,'º',25,6,'À',11,26,'7ÄÙ',25,6,'º',25,2,24,25,2,8,'º'
	db      25,6,14,'filename',25,5,'type',25,6,'úùú module  tit'
	db      'les úùú',25,7,'size',25,5,11,'º',25,2,24,25,2,8,'º',25
	db      2,'Ú',26,13,'Ä',11,'¿  ',8,'ÚÄÄÄ',11,'¿  ',8,'Ú',26,27
	db      'Ä',11,'¿  ',8,'Ú',26,6,'Ä',11,'¿',25,2,'º',25,2

Filemenu_2_Len  equ 59-15
Filemenu_2      label byte
	db      0,19,25,2,8,'º',25,2,'³',16,25,13,11,19,'³  ',8,'³',25
	db      2,11,'³  ',8,'³',25,27,11,'³  ',8,'³',25,6,11,'³',25,2
	db      'º',25,2

Filemenu_3_Len  equ 163-12-8
Filemenu_3      label byte
	db      0,19,25,2,8,'º',25,2,'À',11,26,13,'ÄÙ  ',8,'À',11,'Ä'
	db      'ÄÄÙ  ',8,'À',11,26,27,'ÄÙ  ',8,'À',11,26,6,'ÄÙ',25,2
	db      'º',25,2,24,25,2,8,'È',11,26,'GÍ¼',25,2,24,25,13,'Ú',26
	db      '1Ä',8,'¿',25,13,24,25,13,11,'³ SPACE=tag *=tag all '
	db      'ENTER=load/select ESC=cancel ',8,'³',25,13,24,25,13,11
	db      'À',8,26,'1ÄÙ',25,13,24,25,'O'

FilePath_Len    equ 106-4-15
FilePath        label byte
	db      8,23,25,'O',24,25,2,'É',26,'GÍ',15,'»',25,2,24,25,2,8
	db      'º',25,6,'Ú',26,'=Ä',15,'¿ º',25,2,24,25,2,8,'º ',4,'f'
	db      'ile: ',8,'³',25,'=',15,'³ º',25,2,24,25,2,8,'º',25,6
	db      'À',15,26,'=ÄÙ º',25,2,24,25,2,8,'È',15,26,'GÍ¼',25,2
	db      24,25,'O'

View_1_Len      equ 141-14
View_1          label byte
	db      8,19,25,'O',24,25,2,'É',26,'GÍ',11,'»',25,2,24,25,2,8
	db      'º  ',14,'samp',25,9,'instrument name',25,10,'length'
	db      25,3,'startlp',25,2,'endloop',25,2,11,'º',25,2,24,25,2
	db      8,'º  ÚÄÄ',11,'¿  ',8,'Ú',26,28,'Ä',11,'¿  ',8,'Ú',26
	db      6,'Ä',11,'¿ ',8,'Ú',26,6,'Ä',11,'¿ ',8,'Ú',26,6,'Ä',11
	db      '¿  º',25,2

View_2_Len      equ 63-15
View_2          label byte
	db      11,19,25,2,8,'º  ³  ',11,'³  ',8,'³',25,28,11,'³  ',8
	db      '³',25,6,11,'³ ',8,'³',25,6,11,'³ ',8,'³',25,6,11,'³'
	db      '  º',25,2

View_3_Len      equ 141-12
View_3          label byte
	db      11,19,25,2,8,'º  À',11,'ÄÄÙ  ',8,'À',11,26,28,'ÄÙ  ',8
	db      'À',11,26,6,'ÄÙ ',8,'À',11,26,6,'ÄÙ ',8,'À',11,26,6,'Ä'
	db      'Ù  º',25,2,24,25,2,8,'È',11,26,'GÍ¼',25,2,24,25,23,'Ú'
	db      26,29,'Ä',8,'¿',25,23,24,25,23,11,'³ UP/DOWN=slide l'
	db      'ist  ESC=quit ',8,'³',25,23,24,25,23,11,'À',8,26,29,'Ä'
	db      'Ù',25,23,24,25,'O'

HelpBar_Len     equ 107-15      ;117-12-13   ;101-18-5
HelpBar         label byte
	db      15,18,25,2,14,'F10',15,'=menu  ',14,'C',15,'=cd audi'
	db      'o  ',14,'D',15,'=dos shell  ',14,'Z',15,'=toggle vi'
	db      'deo  ',14,'?',15,'=help  ',14,'ESC',15,'=quit to do'
	db      's',25,2

CD_Screen_Len   equ 397
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

_DATA           ends

;----------------------------------------------------------------------------
; STACK
;----------------------------------------------------------------------------
stack           segment para stack 'STACK'
		db 1000h dup(?)
stack           ends

        end start

