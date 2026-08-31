; CDROMLIB.ASM
; Starplayer MSCDEX CD-Audio library interface code
; Copyright (c) Scott McNab (jedi/oxygen), 1994-1996
; jedi@tartarus.uwa.edu.au

		.386p
		jumps
		locals

;----------------------------------------------------------------------------
code32          segment para public use32
		assume cs:code32, ds:code32

		include pmode.inc

		public CDROM_STATS

		public CD_Head_Position     ;extern unsigned long cd_head_position (void);
		public CD_Get_Volume        ;extern void cd_get_volume (struct volumeinfo *vol);
		public CD_Set_Volume        ;extern void cd_set_volume (struct volumeinfo *vol);
		public CD_GetUPC            ;extern short cd_getupc (void);
		public CD_Get_Audio_Info    ;extern void cd_get_audio_info (void);
		public CD_Set_Track         ;extern void cd_set_track (short tracknum);
		public CD_Track_Length      ;extern void cd_track_length (short tracknum, unsigned char *min, unsigned char *sec, unsigned char *frame);
		public CD_Status            ;extern void cd_status (void);
		public CD_Seek              ;extern void cd_seek (unsigned long location);
		public CD_Play_Audio        ;extern void cd_play_audio (unsigned long begin, unsigned long end);
		public CD_Stop_Audio        ;extern void cd_stop_audio (void);
		public CD_Resume_Audio      ;extern void cd_resume_audio (void);
		public CD_Cmd               ;extern void cd_cmd (unsigned char mode);
		public CD_GetPos            ;extern void cd_getpos (struct playinfo *info);
		public CD_Installed         ;extern short cdrom_installed (void);
		public CD_Done_Play         ;extern short cd_done_play (void);
		public CD_MediaCheck        ;extern short cd_mediach (void);
		public CD_Lock              ;extern void cd_lock (unsigned char doormode);

;----------------------------------------------------------------------------
; Equates and Structures
;----------------------------------------------------------------------------

CDROM           equ     021h            ;local equates
EJECT_TRAY      equ     0
RESET           equ     2
CLOSE_TRAY      equ     5
MEDIA_CHANGE    equ     9
BUSY            equ     512
TRACK_MASK      equ     208

EJECT_TRAY      equ     0               ;public equates
RESET           equ     2
CLOSE_TRAY      equ     5
DATA_TRACK      equ     64
DOORLOCK        equ     1
DOORUNLOCK      equ     0

CDSTAT_DOOROPEN equ     1               ;1=door open, 0=closed
CDSTAT_UNLOCKED equ     2               ;1=door unlocked, 0=locked
CDSTAT_ABILITY  equ     4               ;1=raw and cooked, 0=cooked only
CDSTAT_WRITABLE equ     8               ;1=writable(!), 0=read only
CDSTAT_AUDIOVID equ     16              ;1=can play audio, 0=data only
CDSTAT_PREFETCH equ     128             ;1=supports prefetching, 0=no prefetch
CDSTAT_VOLUME   equ     256             ;1=supports volume control, 0=no vol control
CDSTAT_REDFLAG  equ     512             ;1=RedBook&HSG, 0=HSG only

CD_Playinfo     struc
_control        db      ?
_adr            db      ?
_track          db      ?
_index          db      ?
_min            db      ?
_sec            db      ?
_frame          db      ?
_zero           db      ?
_amin           db      ?
_asec           db      ?
_aframe         db      ?
CD_PlayinfoSize equ     $-_control
CD_Playinfo     ends

CD_Volumeinfo   struc
_mode           db      ?
_input0         db      ?
_volume0        db      ?
_input1         db      ?
_volume1        db      ?
_input2         db      ?
_volume2        db      ?
_input3         db      ?
_volume3        db      ?
CD_VolumeinfoSize equ   $-_mode
CD_Volumeinfo   ends

CD_Cdrom_data   struc
_drives         dw      ?
_first_drive    db      ?
_current_track  dw      ?
_track_position dd      ?
_track_type     db      ?
_low_audio      db      ?
_high_audio     db      ?
_disk_length_min db     ?
_disk_length_sec db     ?
_disk_length_frames db  ?
_endofdisk      dd      ?
_upc            db      7 dup(?)
_diskid         db      6 dup(?)
_status         dd      ?
_error          dw      ?
CD_Cdrom_dataSize equ   $-_drives
CD_Cdrom_data   ends

;----------------------------------------------------------------------------
; DATA
;----------------------------------------------------------------------------

CDROM_STATS     CD_Cdrom_data   ?       ;permanent structure for cdrom stats

Data1           db      32 dup(?)       ;temp data structure 1
Data2           db      32 dup(?)       ;temp data structure 2
TempDataSize    equ     $-Data1

_Temp_Min       db      ?               ;temp variables for HSG routine
_Temp_Sec       db      ?
_Temp_Frame     db      ?

;----------------------------------------------------------------------------
; CODE
;----------------------------------------------------------------------------
tray_request_L  struc           ;structures for CD_Head_Position
L_length        db      ?
L_subunit       db      ?
L_comcode       db      ?
L_status        dw      ?
L_ununsed       db      8 dup(?)
L_media         db      ?
L_address       dd      ?
L_bytes         dw      ?
L_sector        dw      ?
L_volid         dd      ?
L_unused        db      4 dup(?)
tray_request_L_size equ $-L_length
tray_request_L  ends

head_data_L     struc
L_mode          db      ?
L_adr_mode      db      ?
L_address2      dd      ?
head_data_L_size equ $-L_mode
head_data_L     ends

CD_Head_Position proc near      ;extern unsigned long cd_head_position (void);
		call CleanData
		mov esi,offset Data1
		mov [esi.L_length],tray_request_L_size
		;mov [esi.L_subunit],0
		mov [esi.L_comcode],3
		;mov [esi.L_media],0
		;mov [esi.L_sector],0
		;mov [esi.L_volid],0
		mov [esi.L_bytes],6
		mov eax,offset Data2
		call ptr2segoff
		mov word ptr [esi.L_address],ax
		mov word ptr [esi.L_address+2],bx
		mov [Data2.L_mode],01h
		;mov [Data2.L_adr_mode],00h
		mov eax,esi
		call Device_Request
		mov ax,[Data1.L_status]
		mov [CDROM_STATS._error],ax
		mov eax,[Data2.L_address2]
		ret
CD_Head_Position endp

;----------------------------------------------------------------------------
tray_request_G  struc           ;structure for CD_Get_Volume
G_length        db      ?
G_subunit       db      ?
G_comcode       db      ?
G_status        dw      ?
G_ununsed       db      8 dup(?)
G_media         db      ?
G_address       dd      ?
G_bytes         dw      ?
G_sector        dw      ?
G_volid         dd      ?
tray_request_G_size equ $-G_length
tray_request_G  ends

CD_Get_Volume   proc near       ;extern void cd_get_volume (struct volumeinfo *vol);
				;Returns: volumeinfo struc in EAX
		push ebx esi
		call CleanData
		mov [eax._mode],04h
		mov esi,offset Data1
		mov [esi.G_length],tray_request_G_size
		;mov [esi.G_subunit],0
		mov [esi.G_comcode],3
		;mov [esi.G_media],0
		;mov [esi.G_sector],0
		;mov [esi.G_volid],0
		call ptr2segoff
		mov word ptr [esi.G_address],ax
		mov word ptr [esi.G_address+2],bx
		mov [esi.G_bytes],9
		mov eax,offset Data1
		call Device_Request
		mov ax,[Data1.G_status]
		mov [CDROM_STATS._error],ax
		pop esi ebx

		ret
CD_Get_Volume   endp

;----------------------------------------------------------------------------
cd_request_H    struc           ;structure for CD_Set_Volume
H_length        db      ?
H_subunit       db      ?
H_comcode       db      ?
H_status        dw      ?
H_ununsed       db      8 dup(?)
H_media         db      ?
H_address       dd      ?
H_bytes         dw      ?
H_unused        db      4 dup(?)
cd_request_H_size equ $-H_length
cd_request_H  ends

CD_Set_Volume   proc near       ;extern void cd_set_volume (struct volumeinfo *vol);
				;accepts: volumeinfo struc in EAX
		call CleanData

		mov [eax._mode],3
		mov esi,offset Data1
		mov [esi.H_length],cd_request_H_size
		;mov [esi.H_subunit],0
		mov [esi.H_comcode],12
		;mov [esi.H_media],0
		mov [esi.H_bytes],9

		call ptr2segoff
		mov word ptr [esi.H_address],ax
		mov word ptr [esi.H_address+2],bx

		mov eax,offset Data1
		call Device_Request

		mov ax,[Data1.H_status]
		mov [CDROM_STATS._error],ax

		ret
CD_Set_Volume   endp

;----------------------------------------------------------------------------
CD_GetUPC       proc near       ;extern short cd_getupc (void);

		ret
CD_GetUPC       endp


;----------------------------------------------------------------------------
ioctli_A        struc           ;structures for CD_Get_Audio_Info
A_length        db      ?
A_subunit       db      ?
A_comcode       db      ?
A_status        dw      ?
A_ununsed       db      8 dup(?)
A_media         db      ?
A_address       dd      ?
A_bytes         dw      ?
A_sector        dw      ?
A_volid         dd      ?
ioctli_A_size   equ $-A_length
ioctli_A          ends

track_data_A    struc
A_mode          db      ?
A_lowest        db      ?
A_highest       db      ?
A_address2      dd      ?
track_data_A_size equ $-_mode
track_data_A    ends

CD_Get_Audio_Info proc near     ;extern void cd_get_audio_info (void);
		call CleanData

		mov esi,offset Data1
		mov [esi.A_length],ioctli_A_size
		;mov [esi.A_subunit],0
		mov [esi.A_comcode],3
		;mov [esi.A_media],0
		;mov [esi.A_sector],0
		;mov [esi.A_volid],0
		mov [esi.A_bytes],track_data_A_size

		mov eax,offset Data2            ;ptr2segoff (BX:AX)
		call ptr2segoff
		mov word ptr [esi.A_address],ax
		mov word ptr [esi.A_address+2],bx

		mov [Data2.A_mode],0ah

		mov eax,offset Data1            ;device_request (&ioctli);
		call Device_Request

		mov esi,offset CDROM_STATS
		mov eax,dword ptr [Data2.A_lowest]          ;memcpy (&cdrom_data.diskid, &track_data.lowest, 6);
		mov dword ptr [esi._diskid],eax
		mov ax,word ptr [Data2.A_lowest+4]
		mov word ptr [esi._diskid+4],ax

		mov al,[Data2.A_lowest]
		mov [esi._low_audio],al
		mov al,[Data2.A_highest]
		mov [esi._high_audio],al

		mov eax,[Data2.A_address2]      ;red_book (track_data.address, &cdrom_data.disk_length_min, &cdrom_data.disk_length_sec, &cdrom_data.disk_length_frames);
		mov edi,offset CDROM_STATS._disk_length_min
		call RedBook

		mov eax,[Data2.A_address2]      ;cdrom_data.endofdisk = hsg (track_data.address);
		call HSG
		mov [CDROM_STATS._endofdisk],eax

		mov ax,[Data1.A_status]
		mov [CDROM_STATS._error],ax

		ret
CD_Get_Audio_Info endp

;----------------------------------------------------------------------------

tray_request_F  struc           ;structures for CD_Set_Track
F_length        db      ?
F_subunit       db      ?
F_comcode       db      ?
F_status        dw      ?
F_ununsed       db      8 dup(?)
F_media         db      ?
F_address       dd      ?
F_bytes         dw      ?
F_sector        dw      ?
F_volid         dd      ?
tray_request_F_size equ $-F_length
tray_request_F  ends

track_data_F    struc
F_mode          db      ?
F_track         db      ?
F_address2      dd      ?
F_control       db      ?
track_data_F_size equ $-F_mode
track_data_F    ends

CD_Set_Track    proc near       ;extern void cd_set_track (short tracknum);
		push eax ebx ecx
		mov ah,0
		push ax
		call CleanData
		mov esi,offset Data1
		mov [esi.F_length],tray_request_F_size
		;mov [esi.F_subunit],0
		mov [esi.F_comcode],3
		;mov [esi.F_media],0
		;mov [esi.F_sector],0
		;mov [esi.F_volid],0

		mov [esi.F_bytes],7
		mov [Data2.F_mode],0bh
		mov [Data2.F_track],al

		mov eax,offset Data2
		call ptr2segoff
		mov word ptr [esi.F_address],ax
		mov word ptr [esi.F_address+2],bx

		mov eax,esi
		call Device_Request

		mov ax,[Data1.F_status]
		mov [CDROM_STATS._error],ax
		mov eax,[Data2.F_address2]
		call HSG
		mov [CDROM_STATS._track_position],eax
		pop [CDROM_STATS._current_track]

		mov al,[Data2.F_control]
		and al,TRACK_MASK
		mov [CDROM_STATS._track_type],al
		pop ecx ebx eax

		ret
CD_Set_Track    endp

;----------------------------------------------------------------------------
CD_Track_Length proc near       ;extern void cd_track_length (short tracknum, unsigned char *min, unsigned char *sec, unsigned char *frame);
				;get info for track AL, in MIN:SEC:FRM at EDI
		call Get_Track_Length

		;add eax,150
		push eax
		xor edx,edx
		mov ebx,75
		div ebx
		mov [edi+2],dl
		pop eax
		sub eax,edx
		xor edx,edx
		div ebx
		push eax
		xor edx,edx
		mov ebx,60
		div ebx
		mov [edi+1],dl
		pop eax
		sub eax,edx
		xor edx,edx
		div ebx
		mov [edi],al

		ret
CD_Track_Length endp

;----------------------------------------------------------------------------
tray_request_D  struc           ;structures for CD_Status
D_length        db      ?
D_subunit       db      ?
D_comcode       db      ?
D_status        dw      ?
D_ununsed       db      8 dup(?)
D_media         db      ?
D_address       dd      ?
D_bytes         dw      ?
D_sector        dw      ?
D_volid         dd      ?
tray_request_D_size equ $-D_length
tray_request_D  ends

cd_data_D       struc
D_mode          db      ?
D_status2       dd      ?
cd_data_D_size  equ $-D_mode
cd_data_D       ends

CD_Status       proc near       ;extern void cd_status (void);
				;returns: EAX=status flag
				;         BX=error flag
		call CleanData
		mov esi,offset Data1
		mov [esi.D_length],tray_request_D_size
		;mov [esi.D_subunit],0
		mov [esi.D_comcode],3
		;mov [esi.D_media],0
		;mov [esi.D_sector],0
		;mov [esi.D_volid],0
		mov [esi.D_bytes],5

		mov eax,offset Data2            ;tray_request.address = (unsigned long)&cd_data;
		call ptr2segoff
		mov word ptr [esi.D_address],ax
		mov word ptr [esi.D_address+2],bx
		mov [Data2.D_mode],06h
		mov eax,esi
		call Device_Request

		mov eax,[Data2.D_status2]
		mov [CDROM_STATS._status],eax
		mov bx,[Data1.D_status]
		mov [CDROM_STATS._error],bx

		ret
CD_Status       endp

;----------------------------------------------------------------------------
play_request_I  struc near      ;structures for CD_Seek
I_length        db      ?
I_subunit       db      ?
I_comcode       db      ?
I_status        dw      ?
I_ununsed       db      8 dup(?)
I_addressmode   db      ?
I_transfer      dd      ?
I_sectors       dw      ?
I_seekpos       dd      ?
play_request_I_size equ $-I_length
play_request_I  ends

CD_Seek         proc near       ;extern void cd_seek (unsigned long location);
				;accepts: EAX=location
		push ebx ecx
		call CleanData
		mov esi,offset Data1
		mov [esi.I_length],play_request_I_size
		;mov [esi.I_subunit],0
		mov [esi.I_comcode],131
		;mov [esi.I_addressmode],0
		;mov [esi.I_transfer],0
		;mov [esi.I_sectors],0
		mov [esi.I_seekpos],eax
		mov eax,offset Data1
		call Device_Request
		mov ax,[Data1.I_status]
		mov [CDROM_STATS._error],ax
		pop ecx ebx

		ret
CD_Seek         endp

;----------------------------------------------------------------------------
play_request_J  struc           ;structures for CD_Play_Audio
J_length        db      ?
J_subunit       db      ?
J_comcode       db      ?
J_status        dw      ?
J_ununsed       db      8 dup(?)
J_addressmode   db      ?
J_start         dd      ?
J_playlength    dd      ?
play_request_J_size equ $-J_length
play_request_J  ends

CD_Play_Audio   proc near       ;extern void cd_play_audio (unsigned long begin, unsigned long end);
		call CleanData
		mov esi,offset Data1
		mov [esi.J_length],play_request_J_size
		;mov [esi.J_subunit],0
		mov [esi.J_comcode],132
		;mov [esi.J_addressmode],0
		mov [esi.J_start],ebx
		sub ecx,ebx
		mov [esi.J_playlength],ecx
		mov eax,offset Data1
		call Device_Request
		mov ax,[Data1.J_status]
		mov [CDROM_STATS._error],ax

		ret
CD_Play_Audio   endp

;----------------------------------------------------------------------------
stop_request_C  struc           ;structures for CD_Stop_Audio
C_length        db      ?
C_subunit       db      ?
C_comcode       db      ?
C_status        dw      ?
C_ununsed       db      8 dup(?)
stop_request_C_size equ $-C_length
stop_request_C  ends

CD_Stop_Audio   proc near       ;extern void cd_stop_audio (void);
		call CleanData
		mov esi,offset Data1
		mov [esi.C_length],stop_request_C_size
		;mov [esi.C_subunit],0
		mov [esi.C_comcode],133
		mov eax,esi
		call Device_Request
		mov ax,[Data1.C_status]
		mov [CDROM_STATS._error],ax

		ret
CD_Stop_Audio   endp

;----------------------------------------------------------------------------
stop_request_K  struc           ;structure for CD_Resume_Audio
K_length        db      ?
K_subunit       db      ?
K_comcode       db      ?
K_status        dw      ?
K_ununsed       db      8 dup(?)
stop_request_K_size equ $-K_length
stop_request_K  ends

CD_Resume_Audio proc near       ;extern void cd_resume_audio (void);
		call CleanData
		mov esi,offset Data1
		mov [esi.K_length],stop_request_K_size
		;mov [esi.K_subunit],0
		mov [esi.K_comcode],136
		mov eax,esi
		call Device_Request
		mov ax,[Data1.K_status]
		mov [CDROM_STATS._error],ax
		ret
CD_Resume_Audio endp

;----------------------------------------------------------------------------

tray_request_B  struc           ;structures for CD_Cmd
B_length        db      ?
B_subunit       db      ?
B_comcode       db      ?
B_status        dw      ?
B_ununsed       db      8 dup(?)
B_media         db      ?
B_address       dd      ?
B_bytes         dw      ?
B_unused        db      4 dup(?)
tray_request_B_size equ $-B_length
tray_request_B  ends

cd_mode         db      ?       ;temp vaiable for CD_Cmd

CD_Cmd          proc near       ;extern void cd_cmd (unsigned char mode);
				;accepts: AL=mode
		mov [cd_mode],al

		call CleanData
		mov esi,offset Data1
		mov [esi.B_length],tray_request_B_size
		;mov [esi.B_subunit],0
		mov [esi.B_comcode],12
		;mov [esi.B_media],0
		mov eax,offset cd_mode
		call ptr2segoff
		mov word ptr [esi.B_address],ax
		mov word ptr [esi.B_address+2],bx

		mov [esi.B_bytes],1
		mov eax,esi
		call Device_Request

		mov ax,[Data1.B_status]
		mov [CDROM_STATS._error],ax

		ret
CD_Cmd          endp

;----------------------------------------------------------------------------
tray_request_E  struc           ;structures for CD_GetPos
E_length        db      ?
E_subunit       db      ?
E_comcode       db      ?
E_status        dw      ?
E_ununsed       db      8 dup(?)
E_media         db      ?
E_address       dd      ?
E_bytes         dw      ?
E_sector        dw      ?
E_volid         dd      ?
tray_request_E_size equ $-E_length
tray_request_E  ends

CD_GetPos       proc near       ;extern void cd_getpos (struct playinfo *info);
				;accepts: EAX=pointer to playinfo struc
		call CleanData
		mov esi,offset Data1
		mov [esi.E_length],tray_request_E_size
		;mov [esi.E_subunit],0
		mov [esi.E_comcode],3
		;mov [esi.E_media],0
		;mov [esi.E_sector],0
		;mov [esi.E_volid],0
		push eax
		call ptr2segoff
		mov word ptr [esi.E_address],ax
		mov word ptr [esi.E_address+2],bx
		mov [esi.E_bytes],6
		pop ebx
		mov [ebx._control],12
		mov eax,esi
		call Device_Request
		mov ax,[Data1.E_status]
		mov [CDROM_STATS._error],ax

		ret
CD_GetPos       endp

;----------------------------------------------------------------------------
CD_Installed    proc near       ;extern short cdrom_installed (void);
				;returns: CY=1, CD Installed
				;         CY=0, CD not installed


		mov [v86r_ax],1500h     ;check for MSCDEX installed
		mov [v86r_bx],0
		mov al,2fh
		int 33h
		mov ax,[v86r_bx]
		or ax,ax
		jnz @@cdinstalled

		clc
		ret
@@cdinstalled:  mov [CDROM_STATS._drives],ax
		mov ax,[v86r_cx]
		mov [CDROM_STATS._first_drive],al

		call CD_Get_Audio_Info

		stc
		ret
CD_Installed    endp

;----------------------------------------------------------------------------
CD_Done_Play    proc near       ;extern short cd_done_play (void);
				;return: ZF = not busy
				;        NZ = busy

		mov al,CLOSE_TRAY
		call CD_Cmd
		mov ax,[CDROM_STATS._error]
		and ax,BUSY

		ret
CD_Done_Play    endp

;----------------------------------------------------------------------------
CD_MediaCheck   proc near       ;extern short cd_mediach (void);

		ret
CD_MediaCheck   endp

;----------------------------------------------------------------------------
CD_Lock         proc near       ;extern void cd_lock (unsigned char doormode);

		ret
CD_Lock         endp

;----------------------------------------------------------------------------
CleanData       proc near       ;clean temporary data structures
		push edi ecx eax
		mov edi,offset Data1
		mov ecx,TempDataSize/4
		xor eax,eax
		rep stosd
		pop eax ecx edi

		ret
CleanData       endp

;----------------------------------------------------------------------------
Device_Request  proc near       ;Make MSCDEX request with buffer EAX
		push eax
		mov [v86r_ax],1510h
		movzx eax,[CDROM_STATS._first_drive]
		mov [v86r_cx],ax
		pop eax
		call ptr2segoff
		mov [v86r_bx],ax
		mov [v86r_es],bx
		mov al,2fh
		int 33h

		ret
Device_Request  endp

RedBook         proc near       ;take EAX and return min:sec:frames in EDI buffer
		mov [edi+2],al
		shr eax,8
		mov [edi+1],al
		shr eax,8
		mov [edi],al
		ret
RedBook         endp

HSG             proc near       ;take EAX and return HSG size in EAX
		push ebx ecx edx edi
		mov edi,offset _Temp_Min
		call RedBook
		mov eax,4500            ;value = (unsigned long)min * 4500;
		movzx ebx,[_Temp_Min]
		mul ebx
		mov ecx,eax
		mov eax,75              ;value += (short)sec * 75;
		movzx ebx,[_Temp_Sec]
		mul ebx
		add ecx,eax
		movzx eax,[_Temp_Frame] ;value += frame - 150;
		sub eax,150
		add eax,ecx
		pop edi edx ecx ebx
		ret
HSG             endp

Get_Track_Length proc near      ;unsigned long get_track_length (short tracknum)
				;Accepts: AX=track number
		push ebx ecx

		push [CDROM_STATS._current_track]
		call CD_Set_Track
		mov ebx,[CDROM_STATS._track_position]

		cmp al,[CDROM_STATS._high_audio]
		jae @@notend
		inc ax
		call CD_Set_Track
		mov ecx,[CDROM_STATS._track_position]
		jmp short @@gotfin
@@notend:       mov ecx,[CDROM_STATS._endofdisk]
@@gotfin:       sub ecx,ebx             ;ecx=finish, ebx=start
		pop ax
		call CD_Set_Track
		mov eax,ecx
		pop ecx ebx

		ret
Get_Track_Length endp

ptr2segoff      proc near
		add eax,_code32a                ;ptr2segoff (BX:AX)
		shld ebx,eax,28
		and eax,0fh
		ret
ptr2segoff      endp

;----------------------------------------------------------------------------
code32          ends
		end
