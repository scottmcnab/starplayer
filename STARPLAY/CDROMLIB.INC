;Includes for CDROMLIB.ASM pmode MSCDEX audio driver

EJECT_TRAY      equ     0
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

		;extrn CDROM_STATS:NEAR

		extrn CD_Head_Position:NEAR     ;extern unsigned long cd_head_position (void);
		extrn CD_Get_Volume:NEAR        ;extern void cd_get_volume (struct volumeinfo *vol);
		extrn CD_Set_Volume:NEAR        ;extern void cd_set_volume (struct volumeinfo *vol);
		extrn CD_GetUPC:NEAR            ;extern short cd_getupc (void);
		extrn CD_Get_Audio_Info:NEAR    ;extern void cd_get_audio_info (void);
		extrn CD_Set_Track:NEAR         ;extern void cd_set_track (short tracknum);
		extrn CD_Track_Length:NEAR      ;extern void cd_track_length (short tracknum, unsigned char *min, unsigned char *sec, unsigned char *frame);
		extrn CD_Status:NEAR            ;extern void cd_status (void);
		extrn CD_Seek:NEAR              ;extern void cd_seek (unsigned long location);
		extrn CD_Play_Audio:NEAR        ;extern void cd_play_audio (unsigned long begin, unsigned long end);
		extrn CD_Stop_Audio:NEAR        ;extern void cd_stop_audio (void);
		extrn CD_Resume_Audio:NEAR      ;extern void cd_resume_audio (void);
		extrn CD_Cmd:NEAR               ;extern void cd_cmd (unsigned char mode);
		extrn CD_GetPos:NEAR            ;extern void cd_getpos (struct playinfo *info);
		extrn CD_Installed:NEAR         ;extern short cdrom_installed (void);
		extrn CD_Done_Play:NEAR         ;extern short cd_done_play (void);
		extrn CD_MediaCheck:NEAR        ;extern short cd_mediach (void);
		extrn CD_Lock:NEAR              ;extern void cd_lock (unsigned char doormode);
