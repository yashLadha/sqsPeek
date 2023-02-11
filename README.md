### SQSPeek

SQSPeek is a CLI application, that lets you peek messages in SQS.

### Motivation

When debugging messages present in DLQ, I am always frustrated with the default UI
AWS provides. If there are 10 message, not a problem, this becomes a problem when
there are a lot of messages and pagination kicks in.

Secondly, there is no convenient interface to download all the messages that are
present in the SQS. We might need those messages for analysis.

Coming to rescue, `sqsPeek`

### About

`sqsPeek` is a very simple CLI tool, that allows to download the messages present in SQS (Simple Queue Service).

It provides following options:

| Flag               | Type    | Usage                            |
|--------------------|---------|----------------------------------|
| `-d`, `--delete`   | boolean | Purge the messages in DLQ        |
| `-f`, `--fileName` | string  | Output file name                 |
| `-h`, `--help`     | none    | Help menu                        |
| `-p`, `--profile`  | string  | AWS Profile to use to access SQS |
| `-q`, `--queue`    | string  | SQS Queue URL                    |
| `-r`, `--region`   | string  | AWS Region for SQS               |

### Example Usage

**Dump messages from SQS to Local disk**

We can fetch the SQS messages from remote URL and store them on the local file system. Format for the
stored file will be in JSON format, for easy consumption in other programs.

```shell
sqsPeek -q $QUEUE_NAME
```


**Delete messages from SQS**

We can delete the messages from remote URL, if we want to drain it.

```shell
sqsPeek -q $QUEUE_NAME -d
```


### Development

To build the CLI from source, you can perform following steps:

```shell
go build
```