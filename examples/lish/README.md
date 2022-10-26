
This is Lish, the Liso Shell. It shows off many of the features of the Liso crate.

The following builtin commands are available:

- `cd [DIR...]`: Change working directory
- `exit [STATUS]`: Exit Lish
- `help`: This help
- `unsetenv <VARNAME...>`: Blank environment variables
- `setenv <VARNAME=VALUE...>`: Set environment variables
- `jobs`: Show all running jobs

Anything you enter at the shell prompt will either be one of these builtin commands, or will be sent to the operating system for execution as a job. When a new job is created, Lish will switch to it automatically. Use control-X to switch between running jobs, or to return to the Lish prompt to start additional jobs. Press control-C to terminate the currently-selected job. When you're finished playing around, just type `exit` at the shell prompt to quit.

