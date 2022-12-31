package cmd

import (
	"fmt"
	"github.com/spf13/cobra"
	"os"
	"sqsPeek/cmd/execution"
)

var session = execution.RunningSession{}

var rootCmd = &cobra.Command{
	Use:   "sqsPeek",
	Short: "SQS Peek is to peek messages in a SQS",
	Run: func(cmd *cobra.Command, args []string) {
		session.Perform()
	},
}

// Execute the main entry point for CLI.
func Execute() {

	addFlags(&session)
	addFlagValidation()

	if err := rootCmd.Execute(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}

func addFlagValidation() {
	err := rootCmd.MarkPersistentFlagRequired("queue")
	if err != nil {
		println("Queue flag must be passed in the command")
		os.Exit(1)
	}
}

func addFlags(sess *execution.RunningSession) {
	rootCmd.PersistentFlags().StringVarP(&sess.QueueArn, "queue", "q", "", "Queue ARN to purge")
	rootCmd.PersistentFlags().StringVarP(&sess.Region, "region", "r", "ap-south-1", "AWS Region for SQS")
	rootCmd.PersistentFlags().StringVarP(&sess.FileName, "fileName", "f", "queue_messages.json", "File name to store the data")
	rootCmd.PersistentFlags().StringVarP(&sess.Profile, "profile", "p", "DEFAULT", "AWS Profile to access account")
}
